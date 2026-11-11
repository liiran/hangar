import { Folder, Inbox, Layers, PanelLeft, Plus } from "lucide-react";
import type { Session, Status } from "../api";
import { ago, basename, bytes } from "../format";
import { useStore } from "../store";
import { openSession, useUI, useVisible } from "../ui";
import { groups } from "../views";
import { cx, IconButton, Kbd, StatusIcon } from "./ui";

/** The icon each group header shows. */
const GROUP_ICON: Record<string, Status> = { "Needs you": "needs_you", Working: "working", Queued: "queued", Ready: "idle", Paused: "hibernated" };

export function SessionList() {
  const view = useUI((s) => s.view);
  const navHidden = useUI((s) => s.navHidden);
  const toggleNav = useUI((s) => s.toggleNav);
  const open = useUI((s) => s.open);
  const list = useVisible();
  const project = view.kind === "all" ? view.project : null;
  const title = view.kind === "inbox" ? "Needs you" : project ? basename(project) : "Sessions";
  const icon = view.kind === "inbox" ? <Inbox className="size-4" /> : project ? <Folder className="size-4" /> : <Layers className="size-4" />;

  return (
    <section className="flex h-full min-w-0 flex-col" aria-label="Session list">
      <header className="flex h-12 shrink-0 items-center gap-1.5 border-b border-line pr-2 pl-3">
        <IconButton label="Toggle sidebar" shortcut="Ctrl \" onClick={toggleNav} className={cx(!navHidden && "lg:hidden")}>
          <PanelLeft className="size-4" />
        </IconButton>
        <h1 className="flex min-w-0 flex-1 items-center gap-2 px-1 text-[13px] font-medium" title={project ?? undefined}>
          <span className="text-faint">{icon}</span>
          <span className="truncate">{title}</span>
          <span className="text-[12px] font-normal tabular-nums text-faint">{list.length}</span>
        </h1>
        <IconButton label="New session" shortcut="C" onClick={() => open("new")}>
          <Plus className="size-4" />
        </IconButton>
      </header>

      <nav aria-label="Sessions" className="min-h-0 flex-1 overflow-y-auto pb-4">
        {list.length === 0 && <Empty inbox={view.kind === "inbox"} />}
        {groups(list).map(([group, items]) => (
          <section key={group}>
            <h2 className="sticky top-0 z-[1] flex h-8 items-center gap-2 border-b border-line bg-panel-2/80 px-4 text-[12px] font-medium backdrop-blur-sm">
              <StatusIcon status={GROUP_ICON[group] ?? "idle"} size={13} />
              {group}
              <span className="font-normal tabular-nums text-faint">{items.length}</span>
            </h2>
            <ul>
              {items.map((s) => (
                <li key={s.id}>
                  <Row session={s} showProject={!project} />
                </li>
              ))}
            </ul>
          </section>
        ))}
      </nav>
    </section>
  );
}

function Row({ session: s, showProject }: { session: Session; showProject: boolean }) {
  const selected = useStore((st) => st.selected === s.id);
  const sub =
    s.status === "needs_you" && s.pending[0] ? (
      <span className="text-s-needs">Wants to use {s.pending[0].tool_name}</span>
    ) : s.status === "queued" ? (
      s.queue_position != null ? `#${s.queue_position} in line for memory` : "Waiting for memory"
    ) : s.status === "error" ? (
      <span className="text-s-error">{s.status_detail ?? "Stopped with an error"}</span>
    ) : (
      s.preview || "No reply yet"
    );
  return (
    <button
      onClick={() => openSession(s.id)}
      aria-current={selected ? "true" : undefined}
      className={cx("group block w-full border-b border-line px-4 py-2.5 text-left transition-colors", selected ? "bg-raised" : "hover:bg-panel-2/70")}
    >
      <div className="flex items-center gap-2.5">
        <StatusIcon status={s.status} />
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{s.title}</span>
        <span className="shrink-0 text-[11.5px] tabular-nums text-faint">{ago(s.updated_ms)}</span>
      </div>
      <div className="mt-0.5 flex items-center gap-2 pl-[24px] text-[12px] text-muted">
        <span className="min-w-0 flex-1 truncate">
          {showProject && <span className="text-faint">{basename(s.cwd)} · </span>}
          {sub}
        </span>
        {s.memory_bytes != null && <span className="shrink-0 text-[11px] tabular-nums text-faint">{bytes(s.memory_bytes)}</span>}
      </div>
    </button>
  );
}

function Empty({ inbox }: { inbox: boolean }) {
  const open = useUI((s) => s.open);
  return (
    <div className="px-6 py-14 text-center">
      <p className="text-[13px] text-muted">{inbox ? "Nothing needs you right now." : "No sessions here yet."}</p>
      {!inbox && (
        <button onClick={() => open("new")} className="mt-3 inline-flex items-center gap-1.5 text-[12.5px] text-accent hover:underline">
          Start one <Kbd>C</Kbd>
        </button>
      )}
    </div>
  );
}
