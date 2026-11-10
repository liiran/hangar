import { describe, expect, it } from "vitest";
import type { Session, SessionEvent } from "./api";
import { attentionCount, ignoreShortcut, isNewer, mergeEvents, notice, pairCode } from "./sync";

const ev = (seq: number): SessionEvent => ({ seq, ts: seq, kind: "user", text: `m${seq}` });
const session = (over: Partial<Session> = {}): Session => ({
  id: "s1",
  rev: 1,
  title: "t",
  cwd: "C:/w",
  created_ms: 0,
  updated_ms: 0,
  model: null,
  permission_mode: "default",
  claude_session_id: null,
  status: "idle",
  status_detail: null,
  cost_usd: 0,
  turns: 0,
  preview: "",
  peak_bytes: 0,
  pid: null,
  memory_bytes: null,
  pending: [],
  queue_position: null,
  mcp: [],
  ...over,
});

describe("mergeEvents", () => {
  it("appends new events in order and drops duplicates", () => {
    const have = [ev(1), ev(2)];
    const merged = mergeEvents(have, [ev(4), ev(2), ev(3), ev(4)]);
    expect(merged.map((e) => e.seq)).toEqual([1, 2, 3, 4]);
  });

  it("returns the same array when nothing is new, so React skips a render", () => {
    const have = [ev(1)];
    expect(mergeEvents(have, [ev(1)])).toBe(have);
    expect(mergeEvents(have, [])).toBe(have);
  });
});

describe("isNewer", () => {
  it("keeps a fresher snapshot over a stale update that arrives later", () => {
    const snapshot = session({ rev: 10, status: "idle" });
    expect(isNewer(snapshot, session({ rev: 7, status: "working" }))).toBe(false);
    expect(isNewer(snapshot, session({ rev: 11, status: "working" }))).toBe(true);
    expect(isNewer(undefined, session())).toBe(true);
  });
});

describe("pairCode", () => {
  it("reads the one-time code from the fragment", () => {
    expect(pairCode("#pair=0123456789abcdef0123456789abcdef")).toBe("0123456789abcdef0123456789abcdef");
    expect(pairCode("#x=1&pair=abcdef0123456789")).toBe("abcdef0123456789");
  });

  it("ignores anything that is not a code", () => {
    expect(pairCode("")).toBeNull();
    expect(pairCode("#pair=")).toBeNull();
    expect(pairCode("#pair=<script>")).toBeNull();
    expect(pairCode("#repair=abcdef0123456789")).toBeNull();
  });
});

describe("notice", () => {
  it("tells the user when a session needs them, finishes or fails", () => {
    expect(notice(session({ status: "working" }), session({ status: "needs_you" }))).toBe("needs_you");
    expect(notice(session({ status: "working" }), session({ status: "idle" }))).toBe("done");
    expect(notice(session({ status: "working" }), session({ status: "error" }))).toBe("failed");
  });

  it("stays quiet otherwise", () => {
    expect(notice(undefined, session({ status: "needs_you" }))).toBeNull();
    expect(notice(session({ status: "idle" }), session({ status: "idle" }))).toBeNull();
    expect(notice(session({ status: "hibernated" }), session({ status: "idle" }))).toBeNull();
    expect(notice(session({ status: "idle" }), session({ status: "queued" }))).toBeNull();
  });
});

describe("attentionCount", () => {
  it("counts sessions asking for permission or stopped by an error", () => {
    expect(attentionCount([session({ status: "needs_you" }), session({ status: "error" }), session({ status: "working" })])).toBe(2);
  });
});

describe("ignoreShortcut", () => {
  // A stand-in for an element: just what ignoreShortcut looks at.
  const el = (tag: string, opts: { inDialog?: boolean; composer?: boolean } = {}) =>
    ({
      tagName: tag,
      closest: (sel: string) => (opts.inDialog && sel.includes("dialog") ? {} : null),
      hasAttribute: (a: string) => a === "data-composer" && !!opts.composer,
    }) as unknown as EventTarget;
  const key = (target: EventTarget | null, over: Partial<KeyboardEvent> = {}) => ({ isComposing: false, defaultPrevented: false, target, ...over });

  it("lets shortcuts through from the page and the message composer", () => {
    expect(ignoreShortcut(key(el("BODY")))).toBe(false);
    expect(ignoreShortcut(key(el("TEXTAREA", { composer: true })))).toBe(false);
    expect(ignoreShortcut(key(null))).toBe(false);
  });

  it("leaves other fields, dialogs, IME input and handled keys alone", () => {
    expect(ignoreShortcut(key(el("INPUT")))).toBe(true);
    expect(ignoreShortcut(key(el("SELECT")))).toBe(true);
    expect(ignoreShortcut(key(el("TEXTAREA")))).toBe(true);
    expect(ignoreShortcut(key(el("BUTTON", { inDialog: true })))).toBe(true);
    expect(ignoreShortcut(key(el("BODY"), { isComposing: true }))).toBe(true);
    expect(ignoreShortcut(key(el("BODY"), { defaultPrevented: true }))).toBe(true);
  });
});
