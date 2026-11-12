import { ChevronRight, PanelLeft, Server as ServerIcon, Square, Zap } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type Server, type Share, type Unshared } from "../api";
import { bytes } from "../format";
import { report, useStore } from "../store";
import { ignoreShortcut } from "../sync";
import { useUI } from "../ui";
import { Button, cx, IconButton } from "./ui";

const SHARE: Record<Share, { label: string; hint: string; tone: string }> = {
  global: { label: "Shared", hint: "One process for every session", tone: "text-emerald-500 ring-emerald-500/30" },
  project: { label: "Per folder", hint: "Shared by sessions in the same folder", tone: "text-sky-500 ring-sky-500/30" },
  native: { label: "Multi-session", hint: "One process; each session gets its own isolated state (a browser context)", tone: "text-accent ring-accent/35" },
  session: { label: "Per session", hint: "Keeps per-session state, so each session gets its own (still supervised)", tone: "text-muted ring-line-strong" },
  off: { label: "Direct", hint: "Pooling is turned off for this server in config.toml", tone: "text-muted ring-line-strong" },
};

const STATE_DOT: Record<string, string> = { ready: "bg-emerald-500", failed: "bg-s-error" };

export function PoolView() {
  const system = useStore((s) => s.system);
  const navHidden = useUI((s) => s.navHidden);
  const toggleNav = useUI((s) => s.toggleNav);
  const setView = useUI((s) => s.setView);
  const servers = system?.servers ?? [];
  const total = servers.reduce((n, s) => n + s.memory.private_bytes, 0);
  const attachments = servers.reduce((n, s) => n + s.sessions.length, 0);
  // Servers whose sharing was just turned on stay out of the notice until the next update confirms it.
  const [turnedOn, setTurnedOn] = useState<string[]>([]);
  const unshared = (system?.unshared ?? []).filter((s) => !turnedOn.includes(`${s.scope}/${s.name}`));

  // Escape goes back to the sessions, as from any secondary view.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !ignoreShortcut(e)) setView({ kind: "all", project: null });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setView]);

  return (
    <main className="flex min-w-0 flex-1 flex-col" aria-label="MCP pool">
      <header className="flex h-12 shrink-0 items-center gap-1.5 border-b border-line pr-3 pl-3">
        <IconButton label="Toggle sidebar" shortcut="Ctrl \" onClick={toggleNav} className={cx(!navHidden && "lg:hidden")}>
          <PanelLeft className="size-4" />
        </IconButton>
        <h1 className="flex items-center gap-2 px-1 text-[13px] font-medium">
          <ServerIcon className="size-4 text-faint" /> MCP pool
          <span className="text-[12px] font-normal tabular-nums text-faint">{servers.length}</span>
        </h1>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[1000px] px-4 py-6 sm:px-8">
          {unshared.length > 0 && <SharingNotice unshared={unshared} onDone={(keys) => setTurnedOn(keys)} />}
          {unshared.length === 0 && turnedOn.length > 0 && (
            <p role="status" className="mb-6 rounded-lg bg-emerald-500/10 px-4 py-3 text-[12.5px] text-fg ring-1 ring-emerald-500/25">
              Sharing is on for {turnedOn.length === 1 ? "1 server" : `${turnedOn.length} servers`}. Sessions started from now on use it. Sessions that are already running
              keep their own servers until they restart.
            </p>
          )}
          <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg bg-line ring-1 ring-line md:grid-cols-4">
            <Stat label="Servers running" value={String(servers.length)} />
            <Stat label="Session connections" value={String(attachments)} />
            <Stat label="Memory" value={bytes(total)} />
            <Stat label="Saved right now" value={system && system.saved_bytes > 0 ? bytes(system.saved_bytes) : "—"} good={!!system && system.saved_bytes > 0} />
          </div>
          <p className="mt-2 text-[12px] text-faint">Saved is what the extra copies of each shared server would cost if every session started its own, minus what hangar itself uses.</p>

          <div className="mt-6 overflow-hidden rounded-lg ring-1 ring-line">
            <div className="hidden grid-cols-[minmax(0,1fr)_120px_90px_90px_120px_32px] items-center gap-3 border-b border-line bg-panel-2/60 px-4 py-2 text-[11.5px] font-medium text-faint md:grid">
              <span>Server</span>
              <span>Sharing</span>
              <span>Sessions</span>
              <span>Memory</span>
              <span>Requests</span>
              <span />
            </div>
            {servers.length === 0 && (
              <p className="px-4 py-10 text-center text-[12.5px] text-muted">
                {unshared.length > 0
                  ? "No pooled servers are running. Turn on sharing above, and they appear here when a session starts them."
                  : "No pooled servers are running. A local MCP server appears here when a session starts it."}
              </p>
            )}
            {servers.map((s) => (
              <ServerRow key={s.id} server={s} />
            ))}
          </div>

          <p className="mt-4 text-[12px] text-faint">
            hangar shares a server only when every session would see the same behavior. Servers that keep state, and servers it does not know, get their own process per
            session. Set <code className="font-mono">share</code> in config.toml to change that.
          </p>
        </div>
      </div>
    </main>
  );
}

/** Offers what `hangar wrap` does, when Claude Code still starts local servers on its own. */
function SharingNotice({ unshared, onDone }: { unshared: Unshared[]; onDone: (keys: string[]) => void }) {
  const [busy, setBusy] = useState(false);
  const n = unshared.length;
  const names = unshared.map((s) => s.name);
  const shown = names.length > 5 ? `${names.slice(0, 4).join(", ")} and ${names.length - 4} more` : names.join(", ");
  const turnOn = () => {
    setBusy(true);
    api
      .setSharing(true)
      .then(() => onDone(unshared.map((s) => `${s.scope}/${s.name}`)))
      .catch(report)
      .finally(() => setBusy(false));
  };
  return (
    <section aria-label="Sharing is off" className="mb-6 flex flex-col gap-3 rounded-lg bg-accent/8 px-4 py-3.5 ring-1 ring-accent/30 sm:flex-row sm:items-center sm:gap-5">
      <div className="min-w-0 flex-1">
        <h2 className="text-[13px] font-medium">{n === 1 ? "1 local MCP server is not shared yet" : `${n} local MCP servers are not shared yet`}</h2>
        <p className="mt-1 text-[12.5px] leading-relaxed text-muted">
          <span className="font-mono text-[12px] text-fg">{shown}</span>. Every session starts its own copy. With sharing on, sessions use one copy wherever that is safe,
          which saves memory.
        </p>
        <p className="mt-1 text-[12px] leading-relaxed text-faint">
          This changes how those servers start in your Claude Code settings (~/.claude.json) and keeps a backup. It applies to sessions started from now on, here and in a
          terminal. <code className="font-mono">hangar unwrap</code> undoes it.
        </p>
      </div>
      <Button variant="primary" disabled={busy} onClick={turnOn}>
        Turn on sharing
      </Button>
    </section>
  );
}

function Stat({ label, value, good }: { label: string; value: string; good?: boolean }) {
  return (
    <div className="bg-panel px-4 py-3">
      <div className="text-[11.5px] text-faint">{label}</div>
      <div className={cx("mt-0.5 text-[18px] font-semibold tabular-nums tracking-tight", good && "text-emerald-500")}>{value}</div>
    </div>
  );
}

function ServerRow({ server: s }: { server: Server }) {
  const [open, setOpen] = useState(false);
  const share = SHARE[s.share] ?? SHARE.session;
  return (
    <div className="border-b border-line last:border-b-0">
      <div className="group grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-4 py-2.5 hover:bg-panel-2/50 md:grid-cols-[minmax(0,1fr)_120px_90px_90px_120px_32px]">
        <button onClick={() => setOpen(!open)} aria-expanded={open} className="flex min-w-0 items-center gap-2.5 text-left">
          <ChevronRight className={cx("size-3 shrink-0 text-faint transition-transform", open && "rotate-90")} />
          <span className={cx("size-2 shrink-0 rounded-full", STATE_DOT[s.state] ?? "animate-soft-pulse bg-s-working")} title={s.state} />
          <span className="truncate text-[13px] font-medium">{s.name}</span>
          {s.fast_path && <Zap className="size-3 shrink-0 text-s-working" aria-label="fast start" />}
        </button>
        <span className="justify-self-end md:justify-self-start">
          <span className={cx("inline-flex h-5 items-center rounded px-1.5 text-[11px] font-medium ring-1 ring-inset", share.tone)} title={share.hint}>
            {share.label}
          </span>
        </span>
        <span className="pl-[42px] text-[12px] text-muted tabular-nums md:pl-0">
          {s.sessions.length} {s.sessions.length === 1 ? "session" : "sessions"}
        </span>
        <span className="hidden text-[12px] text-muted tabular-nums md:block">{bytes(s.memory.private_bytes)}</span>
        <span className="hidden text-[12px] text-muted tabular-nums md:block">
          {s.requests}
          {s.cache_hits > 0 && <span className="text-faint"> · {s.cache_hits} cached</span>}
        </span>
        <span className="hidden justify-self-end md:block">
          <IconButton label="Stop this server (sessions restart it when needed)" onClick={() => api.stopServer(s.id).catch(report)} className="opacity-0 group-hover:opacity-100 focus:opacity-100">
            <Square className="size-3" />
          </IconButton>
        </span>
      </div>
      {open && (
        <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1.5 bg-panel-2/40 px-4 py-3 pl-[46px] text-[12px]">
          <dt className="text-faint">Command</dt>
          <dd className="truncate font-mono text-[11.5px]" title={s.command}>
            {s.command}
          </dd>
          {s.launch && s.launch !== s.command && (
            <>
              <dt className="text-faint">Started as</dt>
              <dd className="truncate font-mono text-[11.5px]" title={s.launch}>
                {s.launch}
              </dd>
            </>
          )}
          <dt className="text-faint">Memory</dt>
          <dd>
            {bytes(s.memory.private_bytes)} in {s.memory.processes} {s.memory.processes === 1 ? "process" : "processes"}
          </dd>
          <dt className="text-faint">State</dt>
          <dd>
            {s.state}
            {s.pid != null && <span className="text-faint"> · pid {s.pid}</span>}
            {s.restarts > 0 && <span className="text-s-needs"> · restarted {s.restarts}×</span>}
          </dd>
          {s.last_error && (
            <>
              <dt className="text-faint">Last error</dt>
              <dd className="text-s-error">{s.last_error}</dd>
            </>
          )}
        </dl>
      )}
    </div>
  );
}
