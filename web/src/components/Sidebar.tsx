import { Plus, Server } from "lucide-react";
import { useMemo } from "react";
import type { Session } from "../api";
import { ago, basename, bytes, GROUP_ORDER, STATUS } from "../format";
import { useStore } from "../store";
import { cx, Kbd, Meter, StatusDot } from "./ui";

export function Sidebar({ onNew, onPool }: { onNew: () => void; onPool: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const selected = useStore((s) => s.selected);
  const select = useStore((s) => s.select);

  const groups = useMemo(() => {
    const by = new Map<string, Session[]>();
    for (const s of Object.values(sessions)) {
      const g = STATUS[s.status].group;
      if (!by.has(g)) by.set(g, []);
      by.get(g)!.push(s);
    }
    for (const list of by.values()) list.sort((a, b) => b.updated_ms - a.updated_ms);
    return GROUP_ORDER.filter((g) => by.has(g)).map((g) => [g, by.get(g)!] as const);
  }, [sessions]);

  return (
    <aside className="flex h-full w-[300px] shrink-0 flex-col border-r border-line bg-panel">
      <div className="flex items-center justify-between px-4 pt-4 pb-3">
        <div className="flex items-center gap-2">
          <img src="/favicon.svg" className="size-6" alt="" />
          <span className="text-[15px] font-semibold tracking-tight">hangar</span>
        </div>
        <button onClick={onNew} className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-accent px-2.5 text-[13px] font-medium text-accent-fg shadow-sm hover:brightness-110" title="New session (Ctrl+N)">
          <Plus className="size-4" /> New
        </button>
      </div>

      <nav className="min-h-0 flex-1 overflow-y-auto px-2 pb-2" aria-label="Sessions">
        {groups.length === 0 && <p className="px-3 py-6 text-[13px] text-faint">No sessions yet.</p>}
        {groups.map(([group, list]) => (
          <section key={group} className="mb-3">
            <h2 className="flex items-center justify-between px-2.5 pt-2 pb-1 text-[11px] font-medium uppercase tracking-wider text-faint">
              <span>{group}</span>
              <span className="tabular-nums">{list.length}</span>
            </h2>
            <ul className="space-y-0.5">
              {list.map((s) => (
                <li key={s.id}>
                  <button
                    onClick={() => select(s.id)}
                    className={cx(
                      "group w-full rounded-lg px-2.5 py-2 text-left transition-colors",
                      selected === s.id ? "bg-accent-soft ring-1 ring-inset ring-accent/30" : "hover:bg-panel-2",
                    )}
                  >
                    <div className="flex items-center gap-2">
                      <StatusDot status={s.status} />
                      <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{s.title}</span>
                      <span className="shrink-0 text-[11px] tabular-nums text-faint">{ago(s.updated_ms)}</span>
                    </div>
                    <div className="mt-0.5 flex items-center gap-2 pl-4 text-[11.5px] text-faint">
                      <span className="truncate">{basename(s.cwd)}</span>
                      {s.status === "needs_you" && s.pending[0] && <span className="truncate text-amber-500 [@media(prefers-color-scheme:light)]:text-amber-700">· {s.pending[0].tool_name}</span>}
                      {s.status === "queued" && s.queue_position != null && <span>· #{s.queue_position} in line</span>}
                      {s.memory_bytes != null && <span className="ml-auto shrink-0 tabular-nums">{bytes(s.memory_bytes)}</span>}
                    </div>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </nav>

      <MachineCard onPool={onPool} />
    </aside>
  );
}

function MachineCard({ onPool }: { onPool: () => void }) {
  const system = useStore((s) => s.system);
  const sessions = useStore((s) => s.sessions);
  const conn = useStore((s) => s.conn);
  const running = Object.values(sessions).filter((s) => s.pid != null).length;
  const queued = Object.values(sessions).filter((s) => s.status === "queued").length;
  if (!system) return null;
  const m = system.machine;
  const used = m.total_bytes - m.available_bytes;
  const shared = system.servers.filter((s) => s.share !== "session").length;
  return (
    <div className="border-t border-line p-3">
      <div className="rounded-xl bg-panel-2 p-3 ring-1 ring-inset ring-line">
        <div className="mb-2 flex items-baseline justify-between text-[12px]">
          <span className="font-medium">Memory</span>
          <span className="tabular-nums text-muted">
            {bytes(used)} <span className="text-faint">/ {bytes(m.total_bytes)}</span>
          </span>
        </div>
        <Meter used={used} total={m.total_bytes} reserve={m.reserve_bytes} />
        <div className="mt-2 flex justify-between text-[11.5px] text-faint">
          <span>
            {running} running{queued > 0 && <span className="text-slate-300"> · {queued} queued</span>}
          </span>
          <span title="Expected cost of the next session">next ~{bytes(m.estimate_bytes)}</span>
        </div>
        <button onClick={onPool} className="mt-3 flex w-full items-center gap-2 rounded-lg px-1 py-1 text-left text-[12px] hover:bg-raised">
          <Server className="size-3.5 text-muted" />
          <span className="min-w-0 flex-1 truncate">
            MCP pool{" "}
            <span className="text-faint" title={`${shared} shared between sessions`}>
              · {system.servers.length} {system.servers.length === 1 ? "server" : "servers"}
            </span>
          </span>
          {system.saved_bytes > 0 && <span className="rounded bg-emerald-400/10 px-1.5 py-0.5 text-[11px] font-medium text-emerald-400">−{bytes(system.saved_bytes)}</span>}
        </button>
      </div>
      <div className="mt-2 flex items-center justify-between px-1 text-[11px] text-faint">
        <span className="flex items-center gap-1.5">
          <span className={cx("size-1.5 rounded-full", conn === "live" ? "bg-emerald-400" : "bg-amber-400 animate-soft-pulse")} />
          {conn === "live" ? `hangar ${system.version}` : "reconnecting…"}
        </span>
        <span className="flex items-center gap-1">
          <Kbd>Alt</Kbd>
          <Kbd>↑↓</Kbd> switch
        </span>
      </div>
    </div>
  );
}
