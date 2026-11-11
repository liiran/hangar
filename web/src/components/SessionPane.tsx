import { ArrowLeft, ArrowUp, ChevronRight, Copy, Ellipsis, Folder, PanelRight, Pause, Pencil, Play, Square, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, type Session } from "../api";
import { basename, bytes, PERMISSION_MODES, usd } from "../format";
import { report, useStore } from "../store";
import { intent, useUI } from "../ui";
import { Properties } from "./Properties";
import { Transcript } from "./Transcript";
import { Button, cx, IconButton, Kbd, StatusIcon, StatusLabel, useDismiss } from "./ui";

export function SessionPane({ session }: { session: Session }) {
  const events = useStore((s) => s.events[session.id]);
  const props = useUI((s) => s.props);
  const [editing, setEditing] = useState(false);
  return (
    <div className="flex min-w-0 flex-1">
      <main className="flex min-w-0 flex-1 flex-col" aria-label="Session">
        <Toolbar session={session} onRename={() => setEditing(true)} />
        <Transcript session={session} events={events} header={<Title session={session} editing={editing} setEditing={setEditing} />} />
        <Composer session={session} />
      </main>
      {props && (
        <aside className="hidden w-[264px] shrink-0 border-l border-line xl:block" aria-label="Properties">
          <Properties session={session} />
        </aside>
      )}
    </div>
  );
}

function Toolbar({ session, onRename }: { session: Session; onRename: () => void }) {
  const select = useStore((s) => s.select);
  const setView = useUI((s) => s.setView);
  const props = useUI((s) => s.props);
  const toggleProps = useUI((s) => s.toggleProps);
  const act = (f: () => Promise<unknown>) => () => f().catch(report);
  return (
    <header className="flex h-12 shrink-0 items-center gap-1 border-b border-line px-3">
      <IconButton label="Back to the list" onClick={() => select(null)} className="md:hidden">
        <ArrowLeft className="size-4" />
      </IconButton>
      <nav aria-label="Breadcrumb" className="flex min-w-0 flex-1 items-center gap-1 px-1 text-[13px]">
        <button
          onClick={() => setView({ kind: "all", project: session.cwd })}
          className="flex max-w-[45%] min-w-[3rem] shrink-0 items-center gap-1.5 rounded px-1 py-0.5 text-muted hover:bg-panel-2 hover:text-fg"
          title={session.cwd}
        >
          <Folder className="size-3.5 shrink-0" />
          <span className="truncate">{basename(session.cwd)}</span>
        </button>
        <ChevronRight className="size-3.5 shrink-0 text-faint" />
        <span className="flex min-w-0 items-center gap-1.5 px-1">
          <StatusIcon status={session.status} size={13} />
          <span className="truncate font-medium">{session.title}</span>
        </span>
      </nav>
      {session.status === "working" && (
        <Button size="sm" onClick={act(() => api.interrupt(session.id))} title="Stop this turn (Esc)">
          <Square className="size-3 fill-current" /> Stop <Kbd className="ml-0.5 hidden sm:inline-grid">Esc</Kbd>
        </Button>
      )}
      {session.status === "queued" && (
        <Button size="sm" onClick={act(() => api.startNow(session.id))} title="Start now, ignoring the memory budget">
          <Play className="size-3.5" /> Start now
        </Button>
      )}
      <MoreMenu session={session} onRename={onRename} />
      <IconButton label={props ? "Hide properties" : "Show properties"} active={props} onClick={toggleProps} className="hidden xl:grid">
        <PanelRight className="size-4" />
      </IconButton>
    </header>
  );
}

function MoreMenu({ session, onRename }: { session: Session; onRename: () => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(open, ref, () => setOpen(false));
  const live = session.pid != null;
  const act = (f: () => unknown) => () => {
    setOpen(false);
    Promise.resolve(f()).catch(report);
  };
  return (
    <div className="relative" ref={ref}>
      <IconButton label="More" aria-haspopup="menu" aria-expanded={open} active={open} onClick={() => setOpen(!open)}>
        <Ellipsis className="size-4" />
      </IconButton>
      {open && (
        <div role="menu" className="animate-pop absolute right-0 z-30 mt-1 w-56 overflow-hidden rounded-lg bg-panel p-1 shadow-pop">
          <MenuItem icon={<Pencil className="size-3.5" />} onClick={act(onRename)}>
            Rename
          </MenuItem>
          <MenuItem icon={<Copy className="size-3.5" />} onClick={act(() => navigator.clipboard?.writeText(session.cwd))}>
            Copy folder path
          </MenuItem>
          {live && (
            <MenuItem icon={<Pause className="size-3.5" />} onClick={act(() => api.stop(session.id))} hint="resumable">
              Stop process
            </MenuItem>
          )}
          <div className="my-1 border-t border-line" />
          <MenuItem
            icon={<Trash2 className="size-3.5" />}
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
  );
}

function MenuItem({ icon, children, onClick, danger, hint }: { icon: React.ReactNode; children: React.ReactNode; onClick: () => void; danger?: boolean; hint?: string }) {
  return (
    <button
      role="menuitem"
      onClick={onClick}
      className={cx("flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[13px]", danger ? "text-s-error hover:bg-s-error/10" : "text-fg hover:bg-panel-2")}
    >
      <span className={danger ? "" : "text-muted"}>{icon}</span>
      <span className="flex-1">{children}</span>
      {hint && <span className="text-[11px] text-faint">{hint}</span>}
    </button>
  );
}

/** The session's title (click to rename) and, without the properties panel, its key facts. */
function Title({ session, editing, setEditing }: { session: Session; editing: boolean; setEditing: (v: boolean) => void }) {
  const props = useUI((s) => s.props);
  const [title, setTitle] = useState(session.title);
  useEffect(() => setTitle(session.title), [session.title]);
  const save = async () => {
    setEditing(false);
    const t = title.trim();
    if (t && t !== session.title) await api.update(session.id, { title: t }).catch(report);
    else setTitle(session.title);
  };
  const mode = PERMISSION_MODES.find((m) => m.value === session.permission_mode)?.label ?? session.permission_mode;
  return (
    <div className="mb-6">
      {editing ? (
        <input
          autoFocus
          aria-label="Session title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") {
              e.preventDefault();
              setTitle(session.title);
              setEditing(false);
            }
          }}
          className="-mx-1.5 w-[calc(100%+12px)] rounded-md bg-panel-2 px-1.5 py-0.5 text-[20px] font-semibold tracking-tight outline-none ring-1 ring-line-strong"
        />
      ) : (
        <h2 onClick={() => setEditing(true)} className="-mx-1.5 cursor-text rounded-md px-1.5 py-0.5 text-[20px] leading-snug font-semibold tracking-tight hover:bg-panel-2/60" title="Click to rename">
          {session.title}
        </h2>
      )}
      <div className={cx("mt-2 flex flex-wrap items-center gap-1.5 text-[12px] text-muted", props && "xl:hidden")}>
        <Pill>
          <StatusLabel status={session.status} />
        </Pill>
        <Pill>{session.model ?? "Default model"}</Pill>
        <Pill>{mode}</Pill>
        {session.cost_usd > 0 && <Pill>{usd(session.cost_usd)}</Pill>}
        {session.memory_bytes != null && <Pill>{bytes(session.memory_bytes)}</Pill>}
      </div>
    </div>
  );
}

function Pill({ children }: { children: React.ReactNode }) {
  return <span className="inline-flex h-6 items-center rounded-md px-2 ring-1 ring-inset ring-line-strong">{children}</span>;
}

/** Unsent text per session. Outside React: the pane remounts on every switch. */
const drafts = new Map<string, string>();

function Composer({ session }: { session: Session }) {
  const [text, setText] = useState(() => drafts.get(session.id) ?? "");
  const [sending, setSending] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (intent.focusComposer) {
      intent.focusComposer = false;
      ref.current?.focus();
    }
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
    <div className="shrink-0 px-4 pb-4 sm:px-8">
      <div className="mx-auto max-w-[760px]">
        <div className="flex items-end gap-2 rounded-lg bg-panel-2 p-1.5 pl-3 ring-1 ring-inset ring-line-strong transition-shadow focus-within:ring-faint/60">
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
            className="max-h-60 min-h-8 flex-1 resize-none bg-transparent py-1.5 text-[13.5px] leading-relaxed outline-none placeholder:text-faint focus-visible:outline-none"
          />
          <button
            onClick={send}
            disabled={!text.trim() || sending}
            className="grid size-8 shrink-0 place-items-center rounded-md bg-accent text-accent-fg transition disabled:bg-raised disabled:text-faint"
            aria-label="Send"
            title="Send (Enter)"
          >
            <ArrowUp className="size-4" />
          </button>
        </div>
        <div className="mt-1.5 hidden justify-between px-1 text-[11px] text-faint sm:flex">
          <span className="flex items-center gap-1">
            <Kbd>Enter</Kbd> send <span className="mx-1">·</span> <Kbd>Shift</Kbd>
            <Kbd>Enter</Kbd> new line
          </span>
          {session.status === "working" && (
            <span className="flex items-center gap-1">
              <Kbd>Esc</Kbd> stop the turn
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
