import { FolderOpen, Sparkles, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { bytes, MODELS, PERMISSION_MODES } from "../format";
import { askNotificationPermission, report, useStore } from "../store";
import { Button, cx, Kbd } from "./ui";

export function NewSession({ onClose }: { onClose: () => void }) {
  const projects = useStore((s) => s.projects);
  const home = useStore((s) => s.home);
  const system = useStore((s) => s.system);
  const select = useStore((s) => s.select);
  const [cwd, setCwd] = useState(() => {
    try {
      return localStorage.getItem("hangar.lastCwd") ?? projects[0] ?? home;
    } catch {
      return projects[0] ?? home;
    }
  });
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [mode, setMode] = useState("default");
  const [busy, setBusy] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    promptRef.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const m = system?.machine;
  const willQueue = m ? m.available_bytes < m.estimate_bytes + m.reserve_bytes : false;

  const start = async () => {
    if (!cwd.trim() || !prompt.trim() || busy) return;
    setBusy(true);
    askNotificationPermission();
    try {
      const s = await api.create({ cwd: cwd.trim(), prompt, model: model || undefined, permission_mode: mode });
      try {
        localStorage.setItem("hangar.lastCwd", cwd.trim());
      } catch {}
      useStore.setState((st) => ({ sessions: { ...st.sessions, [s.id]: s } }));
      select(s.id);
      onClose();
    } catch (e) {
      report(e);
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/50 p-6 backdrop-blur-[2px]" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div role="dialog" aria-label="New session" className="w-full max-w-xl rounded-2xl bg-panel shadow-2xl ring-1 ring-line">
        <div className="flex items-center justify-between border-b border-line px-5 py-3.5">
          <h2 className="flex items-center gap-2 text-[15px] font-semibold">
            <Sparkles className="size-4 text-accent" /> New session
          </h2>
          <button onClick={onClose} className="rounded-md p-1 text-faint hover:bg-panel-2 hover:text-fg" aria-label="Close">
            <X className="size-4" />
          </button>
        </div>
        <div className="space-y-4 px-5 py-4">
          <label className="block">
            <span className="mb-1.5 block text-[12px] font-medium text-muted">Project folder</span>
            <div className="flex items-center gap-2 rounded-lg bg-panel-2 px-3 ring-1 ring-inset ring-line focus-within:ring-accent/60">
              <FolderOpen className="size-4 shrink-0 text-faint" />
              <input list="hangar-projects" value={cwd} onChange={(e) => setCwd(e.target.value)} spellCheck={false} className="h-9 flex-1 bg-transparent font-mono text-[12.5px] outline-none" placeholder="C:\code\my-app" />
              <datalist id="hangar-projects">
                {projects.map((p) => (
                  <option key={p} value={p} />
                ))}
              </datalist>
            </div>
          </label>
          <label className="block">
            <span className="mb-1.5 block text-[12px] font-medium text-muted">What should Claude do?</span>
            <textarea
              ref={promptRef}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) start();
              }}
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
              <span className="text-slate-300">Starts when ~{bytes(m.estimate_bytes)} frees up; it will wait in the queue.</span>
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
            <Button variant="primary" onClick={start} disabled={!cwd.trim() || !prompt.trim() || busy}>
              {willQueue ? "Queue session" : "Start session"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
