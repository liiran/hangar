import { ChevronRight, Cpu, Shield, ShieldAlert, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type Trust } from "../api";
import { basename, bytes, MODELS, PERMISSION_MODES } from "../format";
import { visible } from "../safe";
import { mergeRecent } from "../folders";
import { askNotificationPermission, report, saved, upsertSession, useStore } from "../store";
import { useUI } from "../ui";
import { projects as sessionFolders } from "../views";
import { FolderPicker } from "./FolderPicker";
import { Select } from "./Select";
import { Button, cx, Kbd, Logo, useDialog } from "./ui";

const LAST_CWD = "hangar.lastCwd";

export function NewSession({ onClose }: { onClose: () => void }) {
  const known = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const home = useStore((s) => s.home);
  // Folders of the sessions on screen come first, so a folder used a moment
  // ago is already there; the daemon's list adds the rest.
  const recent = useMemo(() => mergeRecent(sessionFolders(Object.values(sessions)).map((p) => p.path), known), [sessions, known]);
  const system = useStore((s) => s.system);
  const select = useStore((s) => s.select);
  const view = useUI((s) => s.view);
  // Start in the folder being viewed, else the last one used.
  const [cwd, setCwd] = useState(() => (view.kind === "all" && view.project) || saved.get(LAST_CWD) || recent[0] || home);
  // A new folder is made when the session starts, not when it is picked.
  const [isNew, setIsNew] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [mode, setMode] = useState("default");
  const [busy, setBusy] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const folder = useFolder(cwd, isNew);
  const [trusted, setTrusted] = useState(false);
  useDialog(onClose);

  useEffect(() => promptRef.current?.focus(), []);
  useEffect(() => setTrusted(false), [cwd]);

  const m = system?.machine;
  const willQueue = m ? m.available_bytes < m.estimate_bytes + m.reserve_bytes : false;
  const needsTrust = folder.trust != null && !folder.trust.trusted;
  const ready = cwd.trim() !== "" && prompt.trim() !== "" && !busy && !folder.error && (!needsTrust || trusted);

  const start = async () => {
    if (!ready) return;
    setBusy(true);
    askNotificationPermission();
    try {
      // Trust is given for the notice that was on screen: if the folder's
      // settings changed since, the daemon asks again with the new ones.
      const s = await api.create({ cwd: cwd.trim(), prompt, model: model || undefined, permission_mode: mode, trust: trusted, trust_seen: folder.trust?.fingerprint, create_folder: isNew || undefined });
      saved.set(LAST_CWD, cwd.trim());
      upsertSession(s);
      select(s.id);
      onClose();
    } catch (e) {
      setBusy(false);
      // The folder needs trusting first: show what it would run.
      if (e instanceof ApiError && e.status === 409 && e.data.trust) {
        folder.setTrust(e.data.trust);
        setTrusted(false);
      } else report(e);
    }
  };

  return (
    <div
      className="animate-fade fixed inset-0 z-40 flex items-start justify-center overflow-y-auto bg-black/50 px-4 pt-[12vh] pb-8"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-session-title"
        className="animate-pop w-full max-w-[680px] rounded-xl bg-panel shadow-pop"
        onKeyDown={(e) => {
          // Anywhere in the dialog, not just the prompt box.
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing) {
            e.preventDefault();
            start();
          }
        }}
      >
        <div className="flex items-center gap-1.5 px-4 pt-3.5 text-[12px] text-muted">
          <span className="inline-flex h-6 items-center gap-1.5 rounded-md px-2 ring-1 ring-inset ring-line-strong">
            <Logo size={16} /> hangar
          </span>
          <ChevronRight className="size-3.5 text-faint" />
          <h2 id="new-session-title" className="text-fg">
            New session
          </h2>
          <button onClick={onClose} className="ml-auto grid size-7 place-items-center rounded-md text-faint hover:bg-panel-2 hover:text-fg" aria-label="Close">
            <X className="size-4" />
          </button>
        </div>

        <div className="px-4 pt-3">
          <FolderPicker
            value={cwd}
            isNew={isNew}
            onChange={(p, made) => {
              setCwd(p);
              setIsNew(made);
              promptRef.current?.focus();
            }}
            recent={recent}
            home={home}
            error={folder.error}
          />
          {folder.error && <p className="mt-1 px-1 text-[12px] text-s-error">{folder.error}</p>}
          {isNew && <p className="mt-1 px-1 text-[12px] text-faint">A new, empty folder. hangar makes it when the session starts.</p>}
          <textarea
            ref={promptRef}
            aria-label="What should Claude do?"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={5}
            className="mt-3 w-full resize-none bg-transparent text-[15px] leading-relaxed outline-none placeholder:text-faint"
            placeholder="What should Claude do?"
          />
          {needsTrust && folder.trust && <TrustNotice trust={folder.trust} trusted={trusted} onChange={setTrusted} folder={basename(cwd)} />}
        </div>

        <div className="flex flex-wrap items-center gap-1.5 px-4 pt-1 pb-3">
          <Select icon={<Cpu className="size-3" />} label="Model" value={model} onChange={setModel} options={MODELS} />
          <Select icon={<Shield className="size-3" />} label="Permissions" value={mode} onChange={setMode} options={PERMISSION_MODES} />
          <span className={cx("ml-1 text-[11.5px]", mode === "bypassPermissions" ? "text-s-needs" : "text-faint")}>{PERMISSION_MODES.find((o) => o.value === mode)?.hint}</span>
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-line px-4 py-3">
          <span className="text-[12px] text-faint">
            {willQueue && m ? <span className="text-muted">Needs ~{bytes(m.estimate_bytes)}; it waits in the queue until that is free.</span> : "Claude starts right away."}
          </span>
          <Button variant="primary" size="sm" onClick={start} disabled={!ready}>
            {willQueue ? "Queue session" : "Start session"}
            <Kbd className="ml-1 border-white/25 bg-white/10 text-white/85">Ctrl ⏎</Kbd>
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Looks up what a folder would run (nothing to look up for a new one). */
function useFolder(cwd: string, isNew: boolean) {
  const [trust, setTrust] = useState<Trust | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dir = cwd.trim();
    setTrust(null);
    setError(null);
    if (!dir || isNew) return;
    let current = true;
    const t = setTimeout(() => {
      api
        .trust(dir)
        .then((r) => current && setTrust(r))
        .catch((e) => current && setError(e instanceof ApiError && e.status === 400 ? e.message : null));
    }, 300);
    return () => {
      current = false;
      clearTimeout(t);
    };
  }, [cwd, isNew]);
  return { trust, setTrust, error };
}

function TrustNotice({ trust, trusted, onChange, folder }: { trust: Trust; trusted: boolean; onChange: (v: boolean) => void; folder: string }) {
  const lists = [
    { title: "Hooks it runs", items: trust.hooks },
    { title: "Tools it allows without asking", items: trust.allow_rules },
    { title: "MCP servers it starts", items: trust.mcp_servers },
    { title: "Other settings it changes", items: trust.settings ?? [] },
  ].filter((l) => l.items.length > 0);
  return (
    <div className="mb-3 rounded-lg bg-s-needs/[0.06] p-3.5 text-[12.5px] ring-1 ring-inset ring-s-needs/30">
      <div className="flex items-center gap-2 font-medium">
        <ShieldAlert className="size-4 text-s-needs" /> Trust this folder?
      </div>
      <p className="mt-1 text-muted">Claude Code follows {folder}'s own settings. Start here only if you trust what is in it.</p>
      {lists.length === 0 ? (
        <p className="mt-2 text-faint">hangar found no settings, hooks, pre-approved tools or MCP servers in it. Its files can still carry instructions for Claude (CLAUDE.md, for one).</p>
      ) : (
        lists.map((l) => (
          <div key={l.title} className="mt-2.5">
            <div className="text-[11.5px] text-faint">{l.title}</div>
            {/* Shown whole: nothing here is cut off or hidden behind a scroll bar. */}
            <ul dir="ltr" className="mt-1 space-y-1 rounded-md bg-code px-2.5 py-1.5 font-mono text-[11.5px] [overflow-wrap:anywhere] whitespace-pre-wrap ring-1 ring-inset ring-line">
              {l.items.map((i, n) => (
                <li key={n}>{visible(i)}</li>
              ))}
            </ul>
          </div>
        ))
      )}
      <label className="mt-3 flex cursor-pointer items-center gap-2 font-medium">
        <input type="checkbox" checked={trusted} onChange={(e) => onChange(e.target.checked)} className="size-3.5 accent-[var(--accent)]" />I trust this folder
      </label>
    </div>
  );
}
