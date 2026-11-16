import { Check, Copy, Power } from "lucide-react";
import { useState } from "react";
import { report, stopHangar, useStore } from "../store";
import { Button, Logo, useDialog } from "./ui";

/** Asks before stopping the daemon: it ends every turn that is still running. */
export function StopHangar({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const [busy, setBusy] = useState(false);
  useDialog(onClose);
  const list = Object.values(sessions);
  const midTurn = list.filter((s) => s.status === "working" || s.status === "starting" || s.status === "needs_you").length;
  const running = list.filter((s) => s.pid != null).length;
  const stop = async () => {
    setBusy(true);
    try {
      await stopHangar();
    } catch (e) {
      report(e);
      setBusy(false);
    }
  };
  return (
    <div className="animate-fade fixed inset-0 z-50 flex items-start justify-center bg-black/40 px-4 pt-[18vh]" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div role="dialog" aria-modal="true" aria-labelledby="stop-title" className="animate-pop w-full max-w-[440px] rounded-xl bg-panel p-5 shadow-pop">
        <h2 id="stop-title" className="flex items-center gap-2 text-[15px] font-semibold">
          <Power className="size-4 text-muted" /> Stop hangar?
        </h2>
        <div className="mt-3 space-y-2 text-[13px] text-muted">
          {midTurn > 0 && (
            <p className="rounded-md bg-s-needs/10 px-3 py-2 text-fg ring-1 ring-inset ring-s-needs/30">
              {midTurn === 1 ? "1 session is" : `${midTurn} sessions are`} in the middle of a turn. Stopping ends {midTurn === 1 ? "that turn" : "those turns"} where {midTurn === 1 ? "it is" : "they are"}.
            </p>
          )}
          <p>
            {running > 0 ? `${running === 1 ? "The running session" : `All ${running} running sessions`} will be paused` : "Nothing is running right now"}. Every session keeps its history and picks up where it left off
            on your next message. The shared MCP servers stop too.
          </p>
          <p>
            To start again, run <code className="rounded bg-panel-2 px-1.5 py-0.5 font-mono text-[12px] text-fg">hangar up</code> in a terminal. This is the same as{" "}
            <code className="rounded bg-panel-2 px-1.5 py-0.5 font-mono text-[12px] text-fg">hangar stop</code>.
          </p>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" size="sm" autoFocus onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="danger" size="sm" onClick={stop} disabled={busy}>
            <Power className="size-3.5" /> Stop hangar
          </Button>
        </div>
      </div>
    </div>
  );
}

/** What a window shows once the daemon was stopped on purpose, from here or with `hangar stop`. */
export function StoppedScreen() {
  const [copied, setCopied] = useState(false);
  const copy = () =>
    navigator.clipboard?.writeText("hangar up").then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }, report);
  return (
    <div className="grid h-full place-items-center bg-bg p-8 text-center" data-stopped>
      <div className="max-w-sm">
        <Logo size={36} className="mx-auto mb-5 opacity-60 grayscale" />
        <h1 className="text-[15px] font-semibold">hangar is stopped</h1>
        <p className="mt-1.5 text-[13px] text-muted">Your sessions are paused and keep their history. They pick up where they left off on your next message.</p>
        <p className="mt-5 text-[12.5px] text-faint">To start it again, run this in a terminal:</p>
        <button
          onClick={copy}
          className="mx-auto mt-2 flex items-center gap-2.5 rounded-md bg-panel-2 py-1.5 pr-2.5 pl-3 font-mono text-[12.5px] text-fg ring-1 ring-inset ring-line-strong hover:bg-raised"
          title="Copy the command"
        >
          hangar up
          {copied ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5 text-muted" />}
        </button>
      </div>
    </div>
  );
}
