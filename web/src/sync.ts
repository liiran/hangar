// Pure helpers for keeping the UI in step with the daemon. No DOM, no store:
// everything here is unit tested.

import type { Session, SessionEvent } from "./api";

/** Appends events not seen yet, in sequence order. Event logs are append-only. */
export function mergeEvents(have: SessionEvent[], incoming: SessionEvent[]): SessionEvent[] {
  if (incoming.length === 0) return have;
  const seen = new Set(have.map((e) => e.seq));
  const fresh = incoming.filter((e) => !seen.has(e.seq) && (seen.add(e.seq), true));
  if (fresh.length === 0) return have;
  return have.concat(fresh).sort((a, b) => a.seq - b.seq);
}

/** Whether `next` should replace `prev`: only newer views win. */
export function isNewer(prev: Session | undefined, next: Session): boolean {
  return !prev || next.rev >= prev.rev;
}

/** The one-time pairing code in a `#pair=<code>` URL fragment, if any. */
export function pairCode(hash: string): string | null {
  const m = /^#(?:.*&)?pair=([0-9a-fA-F]{8,64})(?:&|$)/.exec(hash);
  return m ? m[1] : null;
}

export type Notice = "needs_you" | "done" | "failed";

/** What, if anything, a status change should tell the user about. */
export function notice(prev: Session | undefined, next: Session): Notice | null {
  if (!prev || prev.status === next.status) return null;
  if (next.status === "needs_you") return "needs_you";
  if (next.status === "error") return "failed";
  if (next.status === "idle" && (prev.status === "working" || prev.status === "starting")) return "done";
  return null;
}

/** Sessions waiting on the user: asking for permission or failed. */
export function attentionCount(sessions: Iterable<Session>): number {
  let n = 0;
  for (const s of sessions) if (s.status === "needs_you" || s.status === "error") n++;
  return n;
}

/** Keyboard events the app-wide shortcuts must leave alone. */
export function ignoreShortcut(e: Pick<KeyboardEvent, "isComposing" | "defaultPrevented"> & { target: EventTarget | null }): boolean {
  if (e.isComposing || e.defaultPrevented) return true;
  const el = e.target as HTMLElement | null;
  if (!el || typeof el.closest !== "function") return false;
  // Inside a dialog or menu, or a field other than the message composer.
  if (el.closest("[role=dialog],[role=menu]")) return true;
  const tag = el.tagName;
  if (tag === "SELECT" || tag === "INPUT") return true;
  if (tag === "TEXTAREA" && !el.hasAttribute("data-composer")) return true;
  return false;
}
