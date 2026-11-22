import { describe, expect, it } from "vitest";
import type { RewindPreview, SessionEvent } from "./api";
import { canRestoreCode, choiceLines, editedLine, fileNames, restoreProblems, rewindChoices, rewindPoints } from "./rewind";
import { toRows } from "./transcript";

let seq = 0;
const ev = (body: Record<string, unknown>): SessionEvent => ({ seq: ++seq, ts: 1000 + seq, ...body }) as SessionEvent;
const user = (text: string, uuid: string | null = "u") => ev({ kind: "user", text, ...(uuid ? { uuid } : {}) });
const edit = (id: string, name: string, file: string, extra: Record<string, unknown> = {}) => [
  ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "tool_use", id, name, input: { file_path: file } }] }, ...extra } }),
  ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: id, content: "ok" }] }, ...extra } }),
];
const preview = (files: string[], insertions = 0, deletions = 0): RewindPreview => ({ can_restore: true, files, insertions, deletions, error: null });

describe("the messages that can be gone back to", () => {
  it("lists your messages oldest first, each with the files Claude edited while answering it", () => {
    const first = user("fix the cart");
    const events = [
      first,
      ...edit("e1", "Edit", "C:\\app\\src\\cart.ts"),
      ...edit("e2", "Write", "C:\\app\\notes.md"),
      ...edit("e3", "Edit", "C:\\app\\src\\cart.ts"),
      user("thanks"),
      ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "tool_use", id: "r1", name: "Read", input: { file_path: "C:\\app\\a.ts" } }] } } }),
    ];
    const points = rewindPoints(toRows(events), "C:\\app");
    expect(points.map((p) => [p.text, p.seq, p.canRewind, p.edited])).toEqual([
      ["fix the cart", first.seq, true, ["src/cart.ts", "notes.md"]],
      ["thanks", first.seq + 7, true, []],
    ]);
    expect(points.map(editedLine)).toEqual(["Edited 2 files", "No edits"]);
  });

  it("leaves out an edit that failed and the edits of a helper agent, which rewind does not cover", () => {
    const events = [
      user("go"),
      ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "tool_use", id: "x", name: "Edit", input: { file_path: "C:\\app\\bad.ts" } }] } } }),
      ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "x", content: "no", is_error: true }] } } }),
      ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "tool_use", id: "a1", name: "Agent", input: { description: "help" } }] } } }),
      ...edit("h1", "Edit", "C:\\app\\helper.ts", { parent_tool_use_id: "a1" }),
      ...edit("e1", "Edit", "C:\\app\\mine.ts"),
    ];
    const [point] = rewindPoints(toRows(events), "C:\\app");
    expect(point.edited).toEqual(["mine.ts"]);
    expect(editedLine(point)).toBe("Edited mine.ts");
  });

  it("marks a message typed while a turn ran as Claude Code does: no code restore", () => {
    const [point] = rewindPoints(toRows([ev({ kind: "user", text: "and also", uuid: "u9", mid_turn: true })]), "C:\\app");
    expect([point.canRewind, editedLine(point)]).toEqual([true, "⚠ No code restore"]);
  });

  it("offers no message from before Claude summarized the conversation, except one Claude Code kept word for word", () => {
    const events = [
      ev({ kind: "user", text: "long ago", uuid: "u1" }),
      ev({ kind: "user", text: "kept whole", uuid: "u2" }),
      ev({ kind: "claude", msg: { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto", preserved_messages: { all_uuids: ["u2", "x"] } } } }),
      ev({ kind: "user", text: "after", uuid: "u3" }),
    ];
    const points = rewindPoints(toRows(events), "C:\\app");
    expect(points.map((p) => [p.text, p.canRewind, editedLine(p)])).toEqual([
      ["long ago", false, "Before Claude summarized the conversation"],
      ["kept whole", true, "No edits"],
      ["after", true, "No edits"],
    ]);
    // A message sent while the summary was being made is begun after it: it stays.
    const during = [
      ev({ kind: "user", text: "/compact", uuid: "c1" }),
      ev({ kind: "user", text: "sent meanwhile", uuid: "m1" }),
      ev({ kind: "claude", msg: { type: "system", subtype: "compact_boundary", compact_metadata: { preserved_messages: { all_uuids: [] } }, hangar_unstarted: ["m1"] } }),
    ];
    expect(rewindPoints(toRows(during), "C:\\app").map((p) => [p.text, p.canRewind])).toEqual([
      ["/compact", false],
      ["sent meanwhile", true],
    ]);
    // A boundary that names nothing kept (an older Claude Code) leaves nothing before it.
    const bare = [ev({ kind: "user", text: "a", uuid: "u1" }), ev({ kind: "claude", msg: { type: "system", subtype: "compact_boundary" } }), ev({ kind: "user", text: "b", uuid: "u2" })];
    expect(rewindPoints(toRows(bare), "C:\\app").map((p) => p.canRewind)).toEqual([false, true]);
  });

  it("marks a message sent before hangar gave messages an id: it cannot be gone back to", () => {
    const [point] = rewindPoints(toRows([user("old", null)]), "C:\\app");
    expect(point.canRewind).toBe(false);
    expect(editedLine(point)).toBe("Sent before hangar could rewind");
  });
});

describe("the feed after a rewind", () => {
  const story = () => {
    const a = user("one");
    const said = ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "text", text: "did one" }] } } });
    const b = user("two");
    const more = ev({ kind: "claude", msg: { type: "assistant", message: { content: [{ type: "text", text: "did two" }] } } });
    return { a, b, events: [a, said, b, more] };
  };
  const kinds = (events: SessionEvent[]) => toRows(events).map((r) => (r.kind === "user" || r.kind === "text" || r.kind === "rewind" ? `${r.kind}:${r.text}` : r.kind));

  it("drops the message and all after it when the conversation went back, and says so", () => {
    const { b, events } = story();
    expect(kinds([...events, ev({ kind: "rewind", to: b.seq, conversation: true })])).toEqual(["user:one", "text:did one", "rewind:two"]);
    // What is sent afterwards shows after it.
    expect(kinds([...events, ev({ kind: "rewind", to: b.seq, conversation: true }), user("two, better")])).toEqual(["user:one", "text:did one", "rewind:two", "user:two, better"]);
  });

  it("keeps the conversation when only the code was restored", () => {
    const { b, events } = story();
    const rows = toRows([...events, ev({ kind: "rewind", to: b.seq, code: { files: ["C:\\app\\a.ts"], insertions: 1, deletions: 2, skipped: 0 }, conversation: false })]);
    expect(rows.map((r) => r.kind)).toEqual(["user", "text", "user", "text", "rewind"]);
    const last = rows[4];
    expect(last.kind === "rewind" && [last.text, last.conversation, last.code]).toEqual(["two", false, { files: ["C:\\app\\a.ts"], insertions: 1, deletions: 2, skipped: 0 }]);
  });

  it("keeps what was sent while the rewind ran: only rows from the message up to that point go", () => {
    const { b, events } = story();
    const during = user("sent meanwhile");
    const record = ev({ kind: "rewind", to: b.seq, conversation: true, until: during.seq });
    expect(kinds([...events, during, record])).toEqual(["user:one", "text:did one", "user:sent meanwhile", "rewind:two"]);
  });

  it("marks messages that never reached Claude: they cannot be gone back to", () => {
    const sent = user("sent", "u1");
    const dropped = user("dropped", "u2");
    const rows = toRows([sent, dropped, ev({ kind: "unsent", uuids: ["u2"] })]);
    expect(rows.map((r) => r.kind === "user" && r.canRewind)).toEqual([true, false]);
  });

  it("goes back further on a second rewind, taking an earlier rewind's row with it", () => {
    const { a, b, events } = story();
    const twice = [...events, ev({ kind: "rewind", to: b.seq, conversation: true }), user("again"), ev({ kind: "rewind", to: a.seq, conversation: true })];
    expect(kinds(twice)).toEqual(["rewind:one"]);
  });
});

describe("Claude Code's words in the dialog", () => {
  it("offers the two code choices only when the preview found a changed file", () => {
    const labels = (p: RewindPreview | null) => rewindChoices(p).map((c) => c.label);
    expect(labels(preview(["a.ts"], 1, 1))).toEqual(["Restore code and conversation", "Restore conversation", "Restore code", "Never mind"]);
    expect(labels(preview([]))).toEqual(["Restore conversation", "Never mind"]);
    expect(labels({ can_restore: false, files: [], insertions: 0, deletions: 0, error: "No file checkpoint found for this message." })).toEqual(["Restore conversation", "Never mind"]);
    expect(labels(null)).toEqual(["Restore conversation", "Never mind"]);
    expect(canRestoreCode(preview(["a.ts"]))).toBe(true);
  });

  it("says what each choice does to the conversation and to the code", () => {
    const p = preview(["C:\\app\\src\\cart.ts", "C:\\app\\notes.md"], 3, 1);
    expect(choiceLines("both", p)).toEqual(["The conversation goes back to before this message.", "The code will be restored +3 -1 in cart.ts and notes.md."]);
    expect(choiceLines("conversation", p)).toEqual(["The conversation goes back to before this message.", "The code will be unchanged."]);
    expect(choiceLines("code", p)).toEqual(["The conversation will be unchanged.", "The code will be restored +3 -1 in cart.ts and notes.md."]);
    expect(choiceLines("nevermind", p)).toEqual(["The conversation will be unchanged.", "The code will be unchanged."]);
    expect(choiceLines("conversation", preview([]))).toEqual(["The conversation goes back to before this message.", "The code has not changed (nothing will be restored)."]);
  });

  it("names one file, two files, or the first and how many more", () => {
    expect(fileNames(["/app/a.txt"])).toBe("a.txt");
    expect(fileNames(["/app/a.txt", "C:\\app\\b.txt"])).toBe("a.txt and b.txt");
    expect(fileNames(["a.txt", "b.txt", "c.txt", "d.txt"])).toBe("a.txt and 3 other files");
    expect(fileNames([])).toBe("");
  });

  it("reports what did not work after a restore, and nothing when all did", () => {
    expect(restoreProblems({ code: { ok: true, skipped: 0 }, conversation: { ok: true } })).toEqual([]);
    expect(restoreProblems({ conversation: { ok: true } })).toEqual([]);
    expect(restoreProblems({ code: { ok: false, error: "Failed to rewind: X" }, conversation: { ok: true } })).toEqual(["Failed to restore the code: Failed to rewind: X"]);
    expect(restoreProblems({ conversation: { ok: false, error: "target not found" } })).toEqual(["Failed to restore the conversation: target not found"]);
    expect(restoreProblems({ code: { ok: false, error: "A" }, conversation: { ok: false, error: "B" } })).toEqual(["Failed to restore the conversation and code: A B"]);
    expect(restoreProblems({ code: { ok: true, skipped: 2 } })[0]).toMatch(/^Restored the code, but skipped 2 file\(s\): the tracked path is/);
  });
});
