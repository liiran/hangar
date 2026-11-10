import { Gauge, Layers, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import { NewSession } from "./components/NewSession";
import { PoolPanel } from "./components/PoolPanel";
import { SessionPane } from "./components/SessionPane";
import { Sidebar } from "./components/Sidebar";
import { Button, Kbd } from "./components/ui";
import { GROUP_ORDER, STATUS } from "./format";
import { report, useStore } from "./store";
import { attentionCount, ignoreShortcut } from "./sync";

export function App() {
  const conn = useStore((s) => s.conn);
  const authError = useStore((s) => s.authError);
  const sessions = useStore((s) => s.sessions);
  const selected = useStore((s) => s.selected);
  const select = useStore((s) => s.select);
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismiss);
  const [showNew, setShowNew] = useState(false);
  const [showPool, setShowPool] = useState(false);
  const session = selected ? sessions[selected] : undefined;

  // Same order as the sidebar, for Alt+Up/Down.
  const ordered = useMemo(
    () => Object.values(sessions).sort((a, b) => GROUP_ORDER.indexOf(STATUS[a.status].group) - GROUP_ORDER.indexOf(STATUS[b.status].group) || b.updated_ms - a.updated_ms),
    [sessions],
  );

  // Sessions asking for permission or stopped by an error.
  const waiting = attentionCount(Object.values(sessions));
  useEffect(() => {
    document.title = waiting > 0 ? `(${waiting}) hangar` : "hangar";
  }, [waiting]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (showNew || showPool || ignoreShortcut(e)) return;
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        setShowNew(true);
      } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && ordered.length) {
        e.preventDefault();
        const i = ordered.findIndex((s) => s.id === selected);
        const next = e.key === "ArrowDown" ? Math.min(ordered.length - 1, i + 1) : Math.max(0, i - 1);
        select(ordered[next].id);
      } else if (e.key === "Escape" && session?.status === "working") {
        api.interrupt(session.id).catch(report);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ordered, selected, select, session, showNew, showPool]);

  if (conn === "unauthorized") {
    return (
      <Centered>
        <h1 className="text-lg font-semibold">Open hangar from your terminal</h1>
        <p className="mt-2 text-[13.5px] text-muted">
          Run <code className="rounded bg-panel-2 px-1.5 py-0.5 font-mono text-fg">hangar up</code> to sign this window in.
        </p>
        {authError && <p className="mt-3 text-[12.5px] text-faint">{authError}</p>}
      </Centered>
    );
  }
  if (conn === "connecting") {
    return (
      <Centered>
        <p className="text-[13px] text-muted">Connecting to hangar…</p>
      </Centered>
    );
  }

  return (
    <div className="flex h-full">
      <Sidebar onNew={() => setShowNew(true)} onPool={() => setShowPool(true)} />
      {session ? <SessionPane key={session.id} session={session} /> : <Welcome onNew={() => setShowNew(true)} onPool={() => setShowPool(true)} />}
      {showNew && <NewSession onClose={() => setShowNew(false)} />}
      {showPool && <PoolPanel onClose={() => setShowPool(false)} />}
      {conn === "offline" && (
        <div className="fixed top-3 left-1/2 z-50 -translate-x-1/2 rounded-full bg-amber-400/15 px-3 py-1 text-[12px] text-amber-300 ring-1 ring-amber-400/30">Lost the daemon. Reconnecting…</div>
      )}
      <div className="fixed right-4 bottom-4 z-50 space-y-2">
        {toasts.map((t) => (
          <div key={t.id} className="flex max-w-sm items-start gap-3 rounded-xl bg-panel px-4 py-3 text-[13px] shadow-xl ring-1 ring-rose-500/30" role="alert">
            <span className="min-w-0 flex-1 break-words">{t.text}</span>
            <button onClick={() => dismiss(t.id)} className="shrink-0 text-faint hover:text-fg" aria-label="Dismiss">
              ×
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="grid h-full place-items-center p-8 text-center">{<div>{children}</div>}</div>;
}

function Welcome({ onNew, onPool }: { onNew: () => void; onPool: () => void }) {
  const items = [
    { icon: Layers, title: "Every session in one window", text: "See which sessions are working, which are done, and which need you, without switching terminals." },
    { icon: ShieldCheck, title: "Approve from here", text: "Permission requests show up inline with the exact command or diff. Allow once, allow for the session, or deny." },
    { icon: Gauge, title: "Shared MCP servers and a memory budget", text: "One copy of each MCP server for all sessions. New sessions wait for memory instead of slowing everything down." },
  ];
  return (
    <main className="grid flex-1 place-items-center overflow-y-auto p-10">
      <div className="max-w-lg">
        <h1 className="text-2xl font-semibold tracking-tight">Run all your agents from one place</h1>
        <p className="mt-2 text-[14px] text-muted">Start a Claude Code session in any folder. Start as many as you like; hangar keeps your machine responsive.</p>
        <ul className="mt-6 space-y-4">
          {items.map(({ icon: Icon, title, text }) => (
            <li key={title} className="flex gap-3">
              <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-accent-soft text-accent">
                <Icon className="size-4.5" />
              </div>
              <div>
                <div className="text-[14px] font-medium">{title}</div>
                <div className="text-[13px] text-muted">{text}</div>
              </div>
            </li>
          ))}
        </ul>
        <div className="mt-7 flex items-center gap-3">
          <Button variant="primary" onClick={onNew}>
            New session
          </Button>
          <Button variant="ghost" onClick={onPool}>
            View MCP pool
          </Button>
          <span className="text-[12px] text-faint">
            <Kbd>Ctrl</Kbd>+<Kbd>N</Kbd>
          </span>
        </div>
      </div>
    </main>
  );
}
