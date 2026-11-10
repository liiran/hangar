import { Cpu, Square, X, Zap } from "lucide-react";
import { useEffect } from "react";
import { api, type Server } from "../api";
import { bytes } from "../format";
import { report, useStore } from "../store";
import { Button, cx } from "./ui";

const SHARE: Record<Server["share"], { label: string; hint: string; tone: string }> = {
  global: { label: "Shared", hint: "One process for every session", tone: "text-emerald-400 bg-emerald-400/10" },
  project: { label: "Per folder", hint: "Shared by sessions in the same folder", tone: "text-sky-400 bg-sky-400/10" },
  native: { label: "Multi-session", hint: "One process; each session gets its own isolated state (e.g. browser context)", tone: "text-violet-400 bg-violet-400/10" },
  session: { label: "Per session", hint: "Keeps per-session state, so each session gets its own (still supervised)", tone: "text-zinc-400 bg-zinc-400/10" },
};

export function PoolPanel({ onClose }: { onClose: () => void }) {
  const system = useStore((s) => s.system);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const servers = system?.servers ?? [];
  const total = servers.reduce((n, s) => n + s.memory.private_bytes, 0);
  const attachments = servers.reduce((n, s) => n + s.sessions.length, 0);

  return (
    <div className="fixed inset-0 z-30 flex justify-end bg-black/30" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <aside className="flex h-full w-full max-w-md flex-col border-l border-line bg-panel shadow-2xl" aria-label="MCP pool">
        <div className="flex items-start justify-between border-b border-line px-5 py-4">
          <div>
            <h2 className="text-[15px] font-semibold">MCP pool</h2>
            <p className="mt-0.5 text-[12.5px] text-muted">
              {plural(servers.length, "server")} serving {plural(attachments, "session connection")} in {bytes(total)}.
            </p>
          </div>
          <button onClick={onClose} className="rounded-md p-1 text-faint hover:bg-panel-2 hover:text-fg" aria-label="Close">
            <X className="size-4" />
          </button>
        </div>
        {system && system.saved_bytes > 0 && (
          <div className="mx-5 mt-4 rounded-xl bg-emerald-400/[0.08] px-4 py-3 ring-1 ring-inset ring-emerald-400/25">
            <div className="text-[13px] font-medium text-emerald-300 [@media(prefers-color-scheme:light)]:text-emerald-700">About {bytes(system.saved_bytes)} saved right now</div>
            <p className="mt-0.5 text-[12px] text-muted">What the extra copies of each shared server would cost if every session started its own.</p>
          </div>
        )}
        <ul className="min-h-0 flex-1 space-y-2 overflow-y-auto p-5">
          {servers.length === 0 && (
            <li className="rounded-xl border border-dashed border-line-strong p-4 text-[13px] text-muted">
              No pooled servers are running. Run <code className="font-mono text-fg">hangar wrap</code> once so Claude Code starts its stdio MCP servers through hangar.
            </li>
          )}
          {servers.map((s) => (
            <li key={s.id} className="rounded-xl bg-panel-2 p-3.5 ring-1 ring-inset ring-line">
              <div className="flex items-center gap-2">
                <span className={cx("size-2 rounded-full", s.state === "ready" ? "bg-emerald-400" : s.state === "failed" ? "bg-rose-400" : "bg-amber-400 animate-soft-pulse")} title={s.state} />
                <span className="min-w-0 flex-1 truncate text-[13.5px] font-medium">{s.name}</span>
                <span className={cx("rounded px-1.5 py-0.5 text-[11px] font-medium", SHARE[s.share].tone)} title={SHARE[s.share].hint}>
                  {SHARE[s.share].label}
                </span>
                <Button size="sm" variant="ghost" title="Stop this server (sessions will restart it when needed)" onClick={() => api.stopServer(s.id).catch(report)}>
                  <Square className="size-3" />
                </Button>
              </div>
              <div className="mt-2 grid grid-cols-3 gap-2 text-[11.5px]">
                <Stat label="Sessions" value={String(s.sessions.length)} />
                <Stat label="Memory" value={bytes(s.memory.private_bytes)} />
                <Stat label="Requests" value={`${s.requests}${s.cache_hits ? ` · ${s.cache_hits} cached` : ""}`} />
              </div>
              <div className="mt-2 flex items-center gap-1.5 truncate font-mono text-[11px] text-faint" title={s.launch ?? s.command}>
                {s.fast_path ? <Zap className="size-3 shrink-0 text-amber-400" /> : <Cpu className="size-3 shrink-0" />}
                <span className="truncate">{s.fast_path ? "fast start · " : ""}{s.command}</span>
              </div>
              {s.restarts > 0 && <p className="mt-1 text-[11.5px] text-amber-400">Restarted {s.restarts}×{s.last_error ? ` · ${s.last_error}` : ""}</p>}
              {s.state === "failed" && s.last_error && <p className="mt-1 text-[11.5px] text-rose-400">{s.last_error}</p>}
            </li>
          ))}
        </ul>
        <p className="border-t border-line px-5 py-3 text-[11.5px] text-faint">
          hangar shares a server only when every session would see the same behavior. Stateful servers get their own process per session. Unknown servers are shared only within one folder.
        </p>
      </aside>
    </div>
  );
}

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg bg-panel px-2 py-1.5 ring-1 ring-inset ring-line">
      <div className="text-faint">{label}</div>
      <div className="truncate font-medium tabular-nums">{value}</div>
    </div>
  );
}
