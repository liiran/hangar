import { ChevronDown, CircleHelp, Folder, Inbox, Layers, Search, Server, SquarePen } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import { bytes } from "../format";
import { useStore } from "../store";
import { useUI } from "../ui";
import { needsYou, projects, samePath } from "../views";
import { cx, IconButton, Kbd, Logo, Meter } from "./ui";

export function Sidebar() {
  const sessions = useStore((s) => s.sessions);
  const system = useStore((s) => s.system);
  const view = useUI((s) => s.view);
  const setView = useUI((s) => s.setView);
  const open = useUI((s) => s.open);
  const [showProjects, setShowProjects] = useState(true);

  const list = Object.values(sessions);
  const waiting = list.filter(needsYou).length;
  const folders = useMemo(() => projects(Object.values(sessions)).slice(0, 8), [sessions]);
  const project = view.kind === "all" ? view.project : null;

  return (
    <aside className="flex h-full w-full flex-col bg-bg" aria-label="Sidebar">
      <div className="flex h-12 items-center gap-1 px-3">
        <div className="flex min-w-0 flex-1 items-center gap-2 px-1">
          <Logo size={16} />
          <span className="truncate text-[13.5px] font-semibold tracking-tight">hangar</span>
        </div>
        <IconButton label="Search and commands" shortcut="Ctrl K" onClick={() => open("palette")}>
          <Search className="size-4" />
        </IconButton>
        <IconButton label="New session" shortcut="C" onClick={() => open("new")} className="bg-panel-2 text-fg ring-1 ring-inset ring-line hover:bg-raised">
          <SquarePen className="size-[15px]" />
        </IconButton>
      </div>

      <nav aria-label="Main" className="space-y-px px-2">
        <NavItem icon={<Inbox className="size-4" />} active={view.kind === "inbox"} onClick={() => setView({ kind: "inbox" })} count={waiting} highlight={waiting > 0}>
          Needs you
        </NavItem>
        <NavItem icon={<Layers className="size-4" />} active={view.kind === "all" && !project} onClick={() => setView({ kind: "all", project: null })} count={list.length}>
          Sessions
        </NavItem>
        <NavItem
          icon={<Server className="size-4" />}
          active={view.kind === "pool"}
          onClick={() => setView({ kind: "pool" })}
          count={system?.servers.length ?? 0}
          badge={system && system.saved_bytes > 0 ? `−${bytes(system.saved_bytes)}` : undefined}
          badgeTitle="Memory the pool saves right now"
        >
          MCP pool
        </NavItem>
      </nav>

      {folders.length > 0 && (
        <section className="mt-5 min-h-0 flex-1 overflow-y-auto px-2" aria-label="Projects">
          <button onClick={() => setShowProjects(!showProjects)} className="mb-0.5 flex items-center gap-1 rounded px-2 py-1 text-[12px] font-medium text-faint hover:text-muted">
            Projects
            <ChevronDown className={cx("size-3 transition-transform", !showProjects && "-rotate-90")} />
          </button>
          {showProjects && (
            <div className="space-y-px">
              {folders.map((p) => (
                <NavItem
                  key={p.path}
                  icon={<Folder className="size-4" />}
                  active={!!project && samePath(project, p.path)}
                  onClick={() => setView({ kind: "all", project: p.path })}
                  count={p.count}
                  title={p.path}
                >
                  {p.name}
                </NavItem>
              ))}
            </div>
          )}
        </section>
      )}
      {folders.length === 0 && <div className="flex-1" />}

      <MachineFooter />
    </aside>
  );
}

function NavItem({
  icon,
  children,
  active,
  onClick,
  count,
  highlight,
  title,
  badge,
  badgeTitle,
}: {
  icon: ReactNode;
  children: ReactNode;
  active: boolean;
  onClick: () => void;
  count?: number;
  highlight?: boolean;
  title?: string;
  badge?: string;
  badgeTitle?: string;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-current={active ? "page" : undefined}
      className={cx(
        "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] transition-colors",
        active ? "bg-panel-2 font-medium text-fg" : "text-muted hover:bg-panel-2/60 hover:text-fg",
      )}
    >
      <span className={active ? "text-fg" : "text-faint"}>{icon}</span>
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {badge && (
        <span className="rounded bg-emerald-500/12 px-1.5 text-[11px] font-medium whitespace-nowrap text-emerald-500" title={badgeTitle}>
          {badge}
        </span>
      )}
      {!badge && count != null && count > 0 && (
        <span className={cx("text-[11.5px] tabular-nums", highlight ? "rounded bg-s-needs/15 px-1.5 font-medium text-s-needs" : "text-faint")}>{count}</span>
      )}
    </button>
  );
}

function MachineFooter() {
  const system = useStore((s) => s.system);
  const sessions = useStore((s) => s.sessions);
  const conn = useStore((s) => s.conn);
  const open = useUI((s) => s.open);
  if (!system) return null;
  const list = Object.values(sessions);
  const running = list.filter((s) => s.pid != null).length;
  const queued = list.filter((s) => s.status === "queued").length;
  const m = system.machine;
  const used = m.total_bytes - m.available_bytes;
  // How many more sessions can start before new ones wait in the queue.
  const room = Math.max(0, Math.floor((m.available_bytes - m.reserve_bytes) / Math.max(1, m.estimate_bytes)));
  const budget = `A session uses about ${bytes(m.estimate_bytes)}. hangar keeps ${bytes(m.reserve_bytes)} free for the rest of your PC, so a new session waits in the queue (or an idle one is paused) once less than ${bytes(m.estimate_bytes + m.reserve_bytes)} is free.`;
  return (
    <div className="space-y-2.5 border-t border-line px-4 py-3">
      <div>
        <div className="mb-1.5 flex items-baseline justify-between text-[11.5px]" title="Memory in use on this PC, by every program">
          <span className="text-muted">Memory</span>
          <span className="tabular-nums text-faint">
            {bytes(used)} / {bytes(m.total_bytes)}
          </span>
        </div>
        <Meter used={used} total={m.total_bytes} reserve={m.reserve_bytes} />
        <div className="mt-1.5 flex justify-between gap-2 text-[11px] text-faint" title={budget}>
          <span className="truncate">
            {running} running{queued > 0 && ` · ${queued} waiting`}
          </span>
          <span className="shrink-0">{queued > 0 || room === 0 ? "next one waits" : `room for ${room} more ${room === 1 ? "session" : "sessions"}`}</span>
        </div>
      </div>
      <div className="flex items-center justify-between text-[11px] text-faint">
        <span className="flex min-w-0 items-center gap-1.5 whitespace-nowrap">
          <span className={cx("size-1.5 shrink-0 rounded-full", conn === "live" ? "bg-emerald-500" : "animate-soft-pulse bg-s-needs")} />
          <span className="truncate">{conn === "live" ? `hangar ${system.version}` : "reconnecting…"}</span>
        </span>
        <button onClick={() => open("palette")} className="flex shrink-0 items-center gap-1 rounded px-1 py-0.5 hover:bg-panel-2 hover:text-muted" title="Commands and shortcuts">
          <CircleHelp className="size-3.5" />
          <Kbd className="h-4 min-w-4 text-[10px]">Ctrl K</Kbd>
        </button>
      </div>
    </div>
  );
}
