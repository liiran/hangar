import { ArrowUp, Check, ChevronDown, ChevronRight, Clock, Folder, FolderOpen, FolderPlus, HardDrive, House, Search } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { api, type FolderEntry, type Folders } from "../api";
import { basename } from "../format";
import { into, isPathLike, mergeRecent, sections, type FolderItem } from "../folders";
import { cx, Kbd, useDismiss } from "./ui";

/**
 * Picks a project folder the way VS Code does in the browser: search your
 * folders by name, or type or paste a path and its folders appear.
 * (A web page cannot open the system's folder dialog and learn the path.)
 * A new folder is only chosen here; it is made when the session starts.
 */
export function FolderPicker({
  value,
  isNew,
  onChange,
  recent,
  home,
  error,
}: {
  value: string;
  isNew: boolean;
  onChange: (path: string, isNew: boolean) => void;
  recent: string[];
  home: string;
  error?: string | null;
}) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  useDismiss(open, wrap, close);
  return (
    <div ref={wrap} className="relative">
      <button
        ref={trigger}
        type="button"
        aria-label="Choose project folder"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setOpen(true);
          }
        }}
        className={cx(
          "flex h-9 w-full items-center gap-2 rounded-md px-2.5 text-left ring-1 ring-inset transition-colors hover:bg-panel-2",
          error ? "ring-s-error/50" : "ring-line-strong",
          open && "bg-panel-2",
        )}
      >
        {isNew ? <FolderPlus className="size-4 shrink-0 text-accent" /> : <FolderOpen className="size-4 shrink-0 text-muted" />}
        {value ? (
          <>
            <span className="shrink-0 text-[13px] font-medium">{basename(value)}</span>
            {isNew && <span className="shrink-0 rounded bg-accent-soft px-1.5 py-px text-[10.5px] font-medium text-accent">New</span>}
            <span className="min-w-0 flex-1 truncate text-[12px] text-faint">{value}</span>
          </>
        ) : (
          <span className="flex-1 text-[13px] text-faint">Choose a folder…</span>
        )}
        <ChevronDown className="size-3.5 shrink-0 text-faint" />
      </button>
      {open && (
        <Panel
          recent={recent}
          home={home}
          onPick={(path, made) => {
            // The parent moves focus on (to the prompt), so do not take it back.
            setOpen(false);
            onChange(path, made);
          }}
        />
      )}
    </div>
  );
}

const EMPTY: Folders = { dir: null, exists: false, parent: null, entries: [], truncated: false };

function Panel({ recent, home, onPick }: { recent: string[]; home: string; onPick: (path: string, isNew: boolean) => void }) {
  const sep = home.includes("\\") ? "\\" : "/";
  const [query, setQuery] = useState("");
  const [listing, setListing] = useState<{ for: string; data: Folders } | null>(null);
  const [found, setFound] = useState<{ for: string; entries: FolderEntry[] } | null>(null);
  // The daemon's recent folders, read fresh when the picker opens.
  const [fresh, setFresh] = useState<string[] | null>(null);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();
  const q = query.trim();
  const pathy = isPathLike(q);

  // Paths (and the top level) list folders; words search for them by name.
  useEffect(() => {
    let current = true;
    const t = setTimeout(
      () => {
        if (q === "" || pathy) {
          api
            .folders(q)
            .then((data) => {
              if (!current) return;
              setListing({ for: q, data });
              if (data.recent) setFresh(data.recent);
            })
            .catch(() => current && setListing({ for: q, data: EMPTY }));
        } else {
          api
            .searchFolders(q)
            .then((r) => current && setFound({ for: q, entries: r.entries }))
            .catch(() => current && setFound({ for: q, entries: [] }));
        }
      },
      q === "" || pathy ? 90 : 160,
    );
    return () => {
      current = false;
      clearTimeout(t);
    };
  }, [q, pathy]);

  // Only trust answers made for what is typed now.
  const shown = listing && listing.for === q ? listing.data : null;
  const foundNow = found && found.for === q ? found.entries : null;
  const searching = q !== "" && !pathy && !foundNow;
  const waiting = (q === "" || pathy) && !shown;
  const recents = useMemo(() => (fresh ? mergeRecent(fresh, recent) : recent), [fresh, recent]);
  const s = useMemo(() => sections(query, shown, recents, foundNow), [query, shown, recents, foundNow]);
  const items: FolderItem[] = [...s.top, ...s.recent, ...s.found, ...s.folders];

  useEffect(() => setActive(0), [q, shown, foundNow]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const go = (path: string) => {
    setQuery(into(path, sep));
    input.current?.focus();
  };
  const act = (item: FolderItem | undefined, use = false) => {
    if (!item) return;
    if (item.kind === "create") return onPick(item.path, true);
    if (item.kind === "use" || item.kind === "recent" || item.kind === "found" || (use && item.kind === "folder")) return onPick(item.path, false);
    go(item.path);
  };
  const browsable = (item: FolderItem | undefined) => item && (item.kind === "folder" || item.kind === "up" || item.kind === "found" || item.kind === "recent");

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => (items.length ? (i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length : 0));
    } else if (e.key === "Enter" && !e.ctrlKey && !e.metaKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      e.stopPropagation();
      act(items[active]);
    } else if (e.key === "Tab" && !e.shiftKey && browsable(items[active])) {
      e.preventDefault();
      go(items[active].path);
    }
  };

  let index = -1;
  const row = (item: FolderItem, icon: ReactNode, label: ReactNode, detail?: string) => {
    index++;
    const i = index;
    const isActive = i === active;
    return (
      <div
        key={`${item.kind}:${item.path}`}
        id={`${id}-${i}`}
        data-index={i}
        role="option"
        aria-selected={isActive}
        onMouseMove={() => setActive(i)}
        onClick={() => act(item)}
        onDoubleClick={() => item.kind === "folder" && act(item, true)}
        className={cx("group flex h-8 cursor-pointer items-center gap-2.5 rounded-md px-2.5 text-[13px]", isActive ? "bg-panel-2 text-fg" : "text-muted")}
      >
        <span className={cx("shrink-0", isActive ? "text-fg" : "text-faint")}>{icon}</span>
        <span className="min-w-0 truncate">{label}</span>
        {detail ? <span className="min-w-0 flex-1 truncate text-[11.5px] text-faint">{detail}</span> : <span className="flex-1" />}
        {item.kind === "folder" && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onPick(item.path, false);
            }}
            className={cx("rounded px-1.5 py-0.5 text-[11.5px] text-accent hover:bg-accent-soft", isActive ? "visible" : "invisible group-hover:visible")}
          >
            Use
          </button>
        )}
        {(item.kind === "folder" || item.kind === "up") && <ChevronRight className="size-3.5 shrink-0 text-faint" />}
      </div>
    );
  };

  const placeIcon = (name: string) => (name === "Home" ? <House className="size-4" /> : <HardDrive className="size-4" />);
  const parentOf = (p: string) => p.replace(/[\\/][^\\/]*$/, "") || p;

  return (
    <div className="animate-pop absolute inset-x-0 top-full z-50 mt-1 overflow-hidden rounded-lg bg-panel shadow-pop">
      <div className="flex items-center gap-2 border-b border-line px-3">
        <Search className="size-4 shrink-0 text-faint" />
        <input
          ref={input}
          autoFocus
          aria-label="Folder path"
          aria-controls={`${id}-list`}
          aria-activedescendant={items.length ? `${id}-${active}` : undefined}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKey}
          spellCheck={false}
          placeholder={`Search folders by name, or type a path like ${sep === "\\" ? "C:\\code\\app" : "~/code/app"}`}
          className="h-10 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-faint"
        />
        {searching && <span className="size-3 animate-spin rounded-full border-2 border-faint border-t-transparent" aria-label="Searching" />}
      </div>
      <div ref={list} id={`${id}-list`} role="listbox" aria-label="Folders" className="max-h-[320px] overflow-y-auto p-1">
        {s.top.map((item) =>
          item.kind === "use"
            ? row(item, <Check className="size-4 text-accent" />, <span className="font-medium text-fg">Use this folder</span>, item.path)
            : item.kind === "create"
              ? row(item, <FolderPlus className="size-4 text-accent" />, <span className="text-fg">New folder “{item.name}”</span>, "made when the session starts")
              : null,
        )}
        {s.recent.length > 0 && <Heading>Recent</Heading>}
        {s.recent.map((item) => row(item, <Clock className="size-4" />, basename(item.path), item.path))}
        {s.found.length > 0 && <Heading>Folders</Heading>}
        {s.found.map((item) => item.kind === "found" && row(item, <Folder className="size-4" />, item.name, parentOf(item.path)))}
        {s.heading && s.folders.length > 0 && <Heading>{s.heading}</Heading>}
        {s.folders.map((item) =>
          item.kind === "up"
            ? row(item, <ArrowUp className="size-4" />, "Parent folder", item.path)
            : item.kind === "folder"
              ? row(item, shown?.dir == null ? placeIcon(item.name) : <Folder className="size-4" />, item.name, shown?.dir == null ? item.path : undefined)
              : null,
        )}
        {items.length === 0 && (
          <p className="px-3 py-6 text-center text-[12.5px] text-faint">
            {waiting || searching ? "Looking…" : pathy ? "No folders here." : "No folder with that name nearby. Type a full path to browse."}
          </p>
        )}
        {shown?.truncated && <p className="px-3 py-1.5 text-[11.5px] text-faint">Showing the first 300; type more of the name to narrow it down.</p>}
      </div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-line px-3 py-2 text-[11px] text-faint">
        <span className="flex items-center gap-1">
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd> move
        </span>
        <span className="flex items-center gap-1">
          <Kbd>⏎</Kbd> use or open
        </span>
        <span className="flex items-center gap-1">
          <Kbd>Tab</Kbd> look inside
        </span>
        <span className="ml-auto flex items-center gap-1">
          <Kbd>Esc</Kbd> close
        </span>
      </div>
    </div>
  );
}

function Heading({ children }: { children: ReactNode }) {
  return <div className="px-2.5 pt-2 pb-1 text-[11.5px] font-medium text-faint">{children}</div>;
}
