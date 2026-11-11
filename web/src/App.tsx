import { Gauge, Layers, ShieldCheck } from "lucide-react";
import { useEffect, useRef } from "react";
import { api } from "./api";
import { CommandPalette } from "./components/CommandPalette";
import { NewSession } from "./components/NewSession";
import { PoolView } from "./components/PoolView";
import { SessionList } from "./components/SessionList";
import { SessionPane } from "./components/SessionPane";
import { Sidebar } from "./components/Sidebar";
import { Button, cx, Kbd, Logo, useDialog } from "./components/ui";
import { report, useStore } from "./store";
import { attentionCount, ignoreShortcut } from "./sync";
import { focusComposer, useUI, useVisible } from "./ui";
import { step } from "./views";

/** The user is typing somewhere: plain-letter shortcuts stay off. */
function typing(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export function App() {
  const conn = useStore((s) => s.conn);
  const authError = useStore((s) => s.authError);
  const sessions = useStore((s) => s.sessions);
  const selected = useStore((s) => s.selected);
  const select = useStore((s) => s.select);
  const view = useUI((s) => s.view);
  const navHidden = useUI((s) => s.navHidden);
  const navDrawer = useUI((s) => s.navDrawer);
  const overlay = useUI((s) => s.overlay);
  const open = useUI((s) => s.open);
  const toggleNav = useUI((s) => s.toggleNav);
  const closeDrawer = useUI((s) => s.closeDrawer);
  const list = useVisible();
  const session = selected ? sessions[selected] : undefined;

  // Sessions asking for permission or stopped by an error.
  const waiting = attentionCount(Object.values(sessions));
  useEffect(() => {
    document.title = waiting > 0 ? `(${waiting}) hangar` : "hangar";
  }, [waiting]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (overlay !== "new") open(overlay === "palette" ? null : "palette");
        return;
      }
      if (overlay || navDrawer || ignoreShortcut(e)) return;
      if (mod && e.key === "\\") {
        e.preventDefault();
        toggleNav();
      } else if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        open("new");
      } else if (!mod && !e.altKey && !typing(e) && ["c", "j", "k", "r"].includes(e.key)) {
        e.preventDefault();
        if (e.key === "c") open("new");
        else if (e.key === "r") focusComposer();
        else if (view.kind !== "pool") {
          const next = step(list, selected, e.key === "j" ? 1 : -1);
          if (next) select(next);
        }
      } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && view.kind !== "pool") {
        e.preventDefault();
        const next = step(list, selected, e.key === "ArrowDown" ? 1 : -1);
        if (next) select(next);
      } else if (e.key === "Escape" && session?.status === "working" && view.kind !== "pool") {
        api.interrupt(session.id).catch(report);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [overlay, navDrawer, open, toggleNav, list, selected, select, session, view]);

  if (conn === "unauthorized") {
    return (
      <Centered>
        <Logo className="mx-auto mb-5 size-9" />
        <h1 className="text-[15px] font-semibold">Open hangar from your terminal</h1>
        <p className="mt-1.5 text-[13px] text-muted">
          Run <code className="rounded bg-panel-2 px-1.5 py-0.5 font-mono text-[12px] text-fg">hangar up</code> to sign this window in.
        </p>
        {authError && <p className="mt-4 text-[12px] text-faint">{authError}</p>}
      </Centered>
    );
  }
  if (conn === "connecting") {
    return (
      <Centered>
        <Logo className="mx-auto mb-4 size-8 animate-soft-pulse" />
        <p className="text-[12.5px] text-muted">Connecting to hangar…</p>
      </Centered>
    );
  }

  return (
    <div className="flex h-full bg-bg">
      {!navHidden && (
        <div className="hidden w-[232px] shrink-0 lg:block">
          <Sidebar />
        </div>
      )}
      {navDrawer && <Drawer onClose={closeDrawer} />}

      <div className={cx("flex min-w-0 flex-1 lg:py-2 lg:pr-2", navHidden && "lg:pl-2")}>
        <div className="flex min-w-0 flex-1 overflow-hidden bg-panel lg:rounded-lg lg:ring-1 lg:ring-line">
          {view.kind === "pool" ? (
            <PoolView />
          ) : (
            <>
              <div className={cx("w-full shrink-0 border-line md:w-[320px] md:border-r xl:w-[340px]", session ? "hidden md:block" : "block")}>
                <SessionList />
              </div>
              <div className={cx("min-w-0 flex-1", session ? "flex" : "hidden md:flex")}>
                {session ? <SessionPane key={session.id} session={session} /> : <Placeholder empty={Object.keys(sessions).length === 0} />}
              </div>
            </>
          )}
        </div>
      </div>

      {overlay === "new" && <NewSession onClose={() => open(null)} />}
      {overlay === "palette" && <CommandPalette onClose={() => open(null)} />}
      {conn === "offline" && (
        <div className="fixed top-3 left-1/2 z-50 -translate-x-1/2 rounded-full bg-panel px-3 py-1 text-[12px] text-s-needs shadow-pop" role="status">
          Lost the daemon. Reconnecting…
        </div>
      )}
      <Toasts />
    </div>
  );
}

/** The sidebar as a drawer on narrow windows. */
function Drawer({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialog(onClose);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('nav[aria-label="Main"] button')?.focus();
  }, []);
  return (
    <div className="animate-fade fixed inset-0 z-40 bg-black/40 lg:hidden" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label="Navigation" className="animate-slide-left h-full w-[260px] border-r border-line shadow-pop">
        <Sidebar />
      </div>
    </div>
  );
}

function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismiss);
  return (
    <div className="fixed right-4 bottom-4 z-50 space-y-2">
      {toasts.map((t) => (
        <div key={t.id} className="animate-pop flex max-w-sm items-start gap-2.5 rounded-lg bg-panel px-3.5 py-2.5 text-[12.5px] shadow-pop" role="alert">
          <span className="mt-1.5 size-1.5 shrink-0 rounded-full bg-s-error" />
          <span className="min-w-0 flex-1 break-words">{t.text}</span>
          <button onClick={() => dismiss(t.id)} className="shrink-0 text-faint hover:text-fg" aria-label="Dismiss">
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid h-full place-items-center bg-bg p-8 text-center">
      <div>{children}</div>
    </div>
  );
}

/** The detail pane with nothing selected: a welcome, or a nudge. */
function Placeholder({ empty }: { empty: boolean }) {
  const open = useUI((s) => s.open);
  const setView = useUI((s) => s.setView);
  if (!empty) {
    return (
      <div className="grid flex-1 place-items-center p-10 text-center">
        <div className="text-[12.5px] text-faint">
          <p>Select a session to see its activity.</p>
          <p className="mt-3 flex items-center justify-center gap-1.5">
            <Kbd>J</Kbd>
            <Kbd>K</Kbd> to move · <Kbd>R</Kbd> reply · <Kbd>C</Kbd> new · <Kbd>Ctrl K</Kbd> commands
          </p>
        </div>
      </div>
    );
  }
  const items = [
    { icon: Layers, title: "Every session in one window", text: "See which sessions are working, which are done, and which need you, without switching terminals." },
    { icon: ShieldCheck, title: "Approve from here", text: "Permission requests show up inline with the exact command or diff. Allow once, allow for the session, or deny." },
    { icon: Gauge, title: "Shared MCP servers and a memory budget", text: "One copy of each MCP server for all sessions. New sessions wait for memory instead of slowing everything down." },
  ];
  return (
    <div className="grid flex-1 place-items-center overflow-y-auto p-10">
      <div className="max-w-md">
        <Logo className="mb-5 size-8" />
        <h1 className="text-[20px] font-semibold tracking-tight">Run all your agents from one place</h1>
        <p className="mt-1.5 text-[13px] text-muted">Start a Claude Code session in any folder. Start as many as you like; hangar keeps your machine responsive.</p>
        <ul className="mt-6 space-y-4">
          {items.map(({ icon: Icon, title, text }) => (
            <li key={title} className="flex gap-3">
              <span className="grid size-7 shrink-0 place-items-center rounded-md bg-panel-2 text-muted ring-1 ring-inset ring-line">
                <Icon className="size-3.5" />
              </span>
              <div>
                <div className="text-[13px] font-medium">{title}</div>
                <div className="text-[12.5px] text-muted">{text}</div>
              </div>
            </li>
          ))}
        </ul>
        <div className="mt-7 flex items-center gap-2">
          <Button variant="primary" size="sm" onClick={() => open("new")}>
            New session <Kbd className="ml-1 border-white/25 bg-white/10 text-white/85">C</Kbd>
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setView({ kind: "pool" })}>
            View MCP pool
          </Button>
        </div>
      </div>
    </div>
  );
}
