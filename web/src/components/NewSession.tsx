import { FolderOpen, ShieldAlert, Sparkles, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, ApiError, type Trust } from "../api";
import { bytes, MODELS, PERMISSION_MODES } from "../format";
import { askNotificationPermission, report, saved, upsertSession, useStore } from "../store";
import { Button, cx, Kbd, useDialog } from "./ui";

const LAST_CWD = "hangar.lastCwd";

export function NewSession({ onClose }: { onClose: () => void }) {
  const projects = useStore((s) => s.projects);
  const home = useStore((s) => s.home);
  const system = useStore((s) => s.system);
  const select = useStore((s) => s.select);
  const [cwd, setCwd] = useState(() => saved.get(LAST_CWD) ?? projects[0] ?? home);
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [mode, setMode] = useState("default");
  const [busy, setBusy] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const folder = useFolder(cwd);
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
      const s = await api.create({ cwd: cwd.trim(), prompt, model: model || undefined, permission_mode: mode, trust: trusted });
      saved.set(LAST_CWD, cwd.trim());
      upsertSession(s);
      select(s.id);
      onClose();
    } catch (e) {
      setBusy(false);
      // The folder needs trusting first: show what it would run.
      if (e instanceof ApiError && e.status === 409 && e.data.trust) folder.setTrust(e.data.trust);
      else report(e);
    }
  };

  return (
    <div className="fixed inset-0 z-40 grid place-items-center overflow-y-auto bg-black/50 p-6 backdrop-blur-[2px]" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-session-title"
        className="w-full max-w-xl rounded-2xl bg-panel shadow-2xl ring-1 ring-line"
        onKeyDown={(e) => {
          // Anywhere in the dialog, not just the prompt box.
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing) {
            e.preventDefault();
            start();
          }
        }}
      >
        <div className="flex items-center justify-between border-b border-line px-5 py-3.5">
          <h2 id="new-session-title" className="flex items-center gap-2 text-[15px] font-semibold">
            <Sparkles className="size-4 text-accent" /> New session
          </h2>
          <button onClick={onClose} className="rounded-md p-1 text-faint hover:bg-panel-2 hover:text-fg" aria-label="Close">
            <X className="size-4" />
          </button>
        </div>
        <div className="space-y-4 px-5 py-4">
          <label className="block">
            <span className="mb-1.5 block text-[12px] font-medium text-muted">Project folder</span>
            <div className={cx("flex items-center gap-2 rounded-lg bg-panel-2 px-3 ring-1 ring-inset focus-within:ring-accent/60", folder.error ? "ring-rose-500/50" : "ring-line")}>
              <FolderOpen className="size-4 shrink-0 text-faint" />
              <input list="hangar-projects" value={cwd} onChange={(e) => setCwd(e.target.value)} spellCheck={false} className="h-9 flex-1 bg-transparent font-mono text-[12.5px] outline-none" placeholder="C:\code\my-app" />
              <datalist id="hangar-projects">
                {projects.map((p) => (
                  <option key={p} value={p} />
                ))}
              </datalist>
            </div>
            {folder.error && <span className="mt-1 block text-[12px] text-rose-400">{folder.error}</span>}
          </label>
          {needsTrust && folder.trust && <TrustNotice trust={folder.trust} trusted={trusted} onChange={setTrusted} />}
          <label className="block">
            <span className="mb-1.5 block text-[12px] font-medium text-muted">What should Claude do?</span>
            <textarea
              ref={promptRef}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              rows={5}
              className="w-full resize-none rounded-lg bg-panel-2 px-3 py-2.5 text-[14px] outline-none ring-1 ring-inset ring-line placeholder:text-faint focus:ring-accent/60"
              placeholder="Fix the flaky login test and explain the cause"
            />
          </label>
          <div className="grid grid-cols-2 gap-3">
            <label className="block">
              <span className="mb-1.5 block text-[12px] font-medium text-muted">Model</span>
              <select value={model} onChange={(e) => setModel(e.target.value)} className="h-9 w-full rounded-lg bg-panel-2 px-2.5 text-[13px] outline-none ring-1 ring-inset ring-line">
                {MODELS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="block">
              <span className="mb-1.5 block text-[12px] font-medium text-muted">Permissions</span>
              <select value={mode} onChange={(e) => setMode(e.target.value)} className="h-9 w-full rounded-lg bg-panel-2 px-2.5 text-[13px] outline-none ring-1 ring-inset ring-line">
                {PERMISSION_MODES.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <p className={cx("text-[12px]", mode === "bypassPermissions" ? "text-amber-400" : "text-faint")}>{PERMISSION_MODES.find((o) => o.value === mode)?.hint}.</p>
        </div>
        <div className="flex items-center justify-between gap-3 border-t border-line px-5 py-3.5">
          <span className="text-[12px] text-faint">
            {willQueue && m ? (
              <span className="text-muted">Needs ~{bytes(m.estimate_bytes)}; it waits in the queue until that is free.</span>
            ) : (
              <>
                <Kbd>Ctrl</Kbd>+<Kbd>Enter</Kbd> to start
              </>
            )}
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={start} disabled={!ready}>
              {willQueue ? "Queue session" : "Start session"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Looks up what a folder would run, as the user types its path. */
function useFolder(cwd: string) {
  const [trust, setTrust] = useState<Trust | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dir = cwd.trim();
    setTrust(null);
    setError(null);
    if (!dir) return;
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
  }, [cwd]);
  return { trust, setTrust, error };
}

function TrustNotice({ trust, trusted, onChange }: { trust: Trust; trusted: boolean; onChange: (v: boolean) => void }) {
  const lists = [
    { title: "Hooks that run commands", items: trust.hooks },
    { title: "Tools it allows without asking", items: trust.allow_rules },
    { title: "MCP servers it starts", items: trust.mcp_servers },
  ].filter((l) => l.items.length > 0);
  return (
    <div className="rounded-xl bg-amber-400/[0.07] p-3.5 text-[12.5px] ring-1 ring-inset ring-amber-400/30">
      <div className="flex items-center gap-2 font-medium text-amber-300 [@media(prefers-color-scheme:light)]:text-amber-700">
        <ShieldAlert className="size-4" /> New folder
      </div>
      <p className="mt-1 text-muted">Claude Code follows a folder's own settings. Start here only if you trust what is in it.</p>
      {lists.length === 0 ? (
        <p className="mt-2 text-faint">It has no hooks, pre-approved tools or MCP servers of its own.</p>
      ) : (
        lists.map((l) => (
          <div key={l.title} className="mt-2">
            <div className="text-faint">{l.title}</div>
            <ul className="mt-0.5 max-h-28 overflow-y-auto rounded-lg bg-code px-2.5 py-1.5 font-mono text-[11.5px]">
              {l.items.map((i) => (
                <li key={i} className="truncate" title={i}>
                  {i}
                </li>
              ))}
            </ul>
          </div>
        ))
      )}
      <label className="mt-3 flex cursor-pointer items-center gap-2 font-medium">
        <input type="checkbox" checked={trusted} onChange={(e) => onChange(e.target.checked)} className="size-4 accent-[var(--color-accent)]" />I trust this folder
      </label>
    </div>
  );
}
