import { Check, Copy, Download, Inbox, Layers, Monitor, Moon, PanelLeft, PanelRight, Play, Search, Server, Square, SquarePen, Sun } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api } from "../api";
import { basename } from "../format";
import { report, useStore } from "../store";
import { THEMES, useInstall, useTheme } from "../theme";
import { openSession, useUI } from "../ui";
import { matches } from "../views";
import { cx, Kbd, StatusIcon, useDialog } from "./ui";

interface Item {
  id: string;
  group: string;
  label: string;
  detail?: string;
  icon: ReactNode;
  shortcut?: string;
  run: () => void;
}

export function CommandPalette({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const selected = useStore((s) => (s.selected ? s.sessions[s.selected] : undefined));
  const ui = useUI();
  const theme = useTheme();
  const install = useInstall();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  useDialog(onClose);

  const items = useMemo(() => {
    const actions: Item[] = [
      { id: "new", group: "Actions", label: "New session", icon: <SquarePen className="size-4" />, shortcut: "C", run: () => ui.open("new") },
      { id: "inbox", group: "Actions", label: "Go to Needs you", icon: <Inbox className="size-4" />, run: () => ui.setView({ kind: "inbox" }) },
      { id: "all", group: "Actions", label: "Go to all sessions", icon: <Layers className="size-4" />, run: () => ui.setView({ kind: "all", project: null }) },
      { id: "pool", group: "Actions", label: "Go to MCP pool", icon: <Server className="size-4" />, run: () => ui.setView({ kind: "pool" }) },
      { id: "nav", group: "Actions", label: "Toggle sidebar", icon: <PanelLeft className="size-4" />, shortcut: "Ctrl \\", run: ui.toggleNav },
      { id: "props", group: "Actions", label: "Toggle properties panel", icon: <PanelRight className="size-4" />, run: ui.toggleProps },
    ];
    // After the sessions, so the list still opens on what is used most.
    const themeIcons = { system: Monitor, light: Sun, dark: Moon };
    const appearance: Item[] = THEMES.map((t) => {
      const Icon = theme.choice === t.value ? Check : themeIcons[t.value];
      return { id: `theme-${t.value}`, group: "Appearance", label: `Theme: ${t.label}`, detail: theme.choice === t.value ? "current" : undefined, icon: <Icon className="size-4" />, run: () => theme.choose(t.value) };
    });
    if (install.offer) {
      appearance.push({ id: "install", group: "Appearance", label: "Install hangar as an app", detail: "its own taskbar icon and window", icon: <Download className="size-4" />, run: install.install });
    }
    if (selected) {
      const s = selected;
      if (s.status === "working")
        actions.unshift({ id: "stop", group: "Actions", label: "Stop the turn", detail: s.title, icon: <Square className="size-3.5" />, shortcut: "Esc", run: () => api.interrupt(s.id).catch(report) });
      if (s.status === "queued")
        actions.unshift({ id: "start", group: "Actions", label: "Start now", detail: s.title, icon: <Play className="size-4" />, run: () => api.startNow(s.id).catch(report) });
      actions.push({ id: "copy", group: "Actions", label: "Copy folder path", detail: basename(s.cwd), icon: <Copy className="size-4" />, run: () => navigator.clipboard?.writeText(s.cwd) });
    }
    const all = Object.values(sessions).sort((a, b) => b.updated_ms - a.updated_ms);
    const found: Item[] = all
      .filter((s) => matches(query, s.title, s.cwd, s.preview))
      .slice(0, query ? 12 : 6)
      .map((s) => ({ id: s.id, group: query ? "Sessions" : "Recent sessions", label: s.title, detail: basename(s.cwd), icon: <StatusIcon status={s.status} />, run: () => openSession(s.id) }));
    return [...actions.filter((a) => matches(query, a.label, a.detail)), ...found, ...appearance.filter((a) => matches(query, a.label, a.detail))];
  }, [query, sessions, selected, ui, theme, install]);

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const run = (item: Item | undefined) => {
    if (!item) return;
    onClose();
    item.run();
  };

  let lastGroup = "";
  return (
    <div className="animate-fade fixed inset-0 z-50 flex items-start justify-center bg-black/40 px-4 pt-[14vh]" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div role="dialog" aria-modal="true" aria-label="Command palette" className="animate-pop w-full max-w-[620px] overflow-hidden rounded-xl bg-panel shadow-pop">
        <div className="flex items-center gap-2.5 border-b border-line px-4">
          <Search className="size-4 text-faint" />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
                e.preventDefault();
                setActive((i) => (items.length ? (i + 1) % items.length : 0));
              } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
                e.preventDefault();
                setActive((i) => (items.length ? (i - 1 + items.length) % items.length : 0));
              } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                e.preventDefault();
                run(items[active]);
              }
            }}
            placeholder="Type a command or search sessions…"
            aria-label="Command or session"
            aria-controls="palette-list"
            className="h-12 flex-1 bg-transparent text-[14px] outline-none placeholder:text-faint"
          />
        </div>
        <div ref={list} id="palette-list" role="listbox" className="max-h-[min(380px,55vh)] overflow-y-auto p-1.5">
          {items.length === 0 && <p className="px-3 py-6 text-center text-[12.5px] text-faint">Nothing matches “{query}”.</p>}
          {items.map((item, i) => {
            const heading = item.group !== lastGroup ? item.group : null;
            lastGroup = item.group;
            return (
              <div key={`${item.group}-${item.id}`}>
                {heading && <div className="px-2.5 pt-2 pb-1 text-[11.5px] font-medium text-faint">{heading}</div>}
                <button
                  role="option"
                  aria-selected={i === active}
                  data-index={i}
                  onMouseMove={() => setActive(i)}
                  onClick={() => run(item)}
                  className={cx("flex h-9 w-full items-center gap-3 rounded-md px-2.5 text-left text-[13px]", i === active ? "bg-panel-2 text-fg" : "text-muted")}
                >
                  <span className={cx("grid w-4 place-items-center", i === active ? "text-fg" : "text-faint")}>{item.icon}</span>
                  <span className="min-w-0 truncate">{item.label}</span>
                  {item.detail && <span className="min-w-0 flex-1 truncate text-[12px] text-faint">{item.detail}</span>}
                  {!item.detail && <span className="flex-1" />}
                  {item.shortcut && <Kbd>{item.shortcut}</Kbd>}
                </button>
              </div>
            );
          })}
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-line px-4 py-2 text-[11px] text-faint">
          <span className="flex items-center gap-1">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> move
          </span>
          <span className="flex items-center gap-1">
            <Kbd>⏎</Kbd> open
          </span>
          <span className="flex items-center gap-1">
            <Kbd>J</Kbd>
            <Kbd>K</Kbd> next / previous session
          </span>
          <span className="flex items-center gap-1">
            <Kbd>C</Kbd> new session
          </span>
          <span className="flex items-center gap-1">
            <Kbd>R</Kbd> reply
          </span>
          <span className="ml-auto flex items-center gap-1">
            <Kbd>Esc</Kbd> close
          </span>
        </div>
      </div>
    </div>
  );
}
