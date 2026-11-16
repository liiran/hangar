import { describe, expect, it } from "vitest";
import { diffOf, fromContent, fromEdit, fromPatch } from "./diff";

const shape = (lines: { sign: string; text: string; n: number | null }[]) => lines.map((l) => `${l.n ?? ""}${l.sign}${l.text}`);

describe("fromPatch", () => {
  it("numbers lines the way the file does: added and unchanged lines after the change, removed lines before it", () => {
    // What Claude Code 2.1.325 reported for replacing "line 17" with two lines.
    const d = fromPatch({ structuredPatch: [{ oldStart: 14, newStart: 14, lines: [" line 14", " line 15", " line 16", "-line 17", "+LINE SEVENTEEN", "+an extra line", " line 18"] }] })!;
    expect(shape(d.hunks[0])).toEqual(["14 line 14", "15 line 15", "16 line 16", "17-line 17", "17+LINE SEVENTEEN", "18+an extra line", "19 line 18"]);
    expect([d.added, d.removed, d.truncated]).toEqual([2, 1, false]);
  });

  it("keeps each changed stretch apart and passes on that the patch was cut", () => {
    const d = fromPatch({
      truncated: true,
      structuredPatch: [
        { oldStart: 1, newStart: 1, lines: ["-a", "+b"] },
        { oldStart: 88, newStart: 88, lines: [" x", "-old", "+new"] },
      ],
    })!;
    expect(d.hunks.map(shape)).toEqual([
      ["1-a", "1+b"],
      ["88 x", "89-old", "89+new"],
    ]);
    expect(d.truncated).toBe(true);
  });

  it("shows the patch's own notes without a number, and they do not shift the numbers after them", () => {
    const d = fromPatch({ structuredPatch: [{ oldStart: 1, newStart: 1, lines: [" {", '-  "port": 8080', "\\ No newline at end of file", '+  "port": 8080,', "+}"] }] })!;
    expect(shape(d.hunks[0])).toEqual(["1 {", '2-  "port": 8080', "\\No newline at end of file", '2+  "port": 8080,', "3+}"]);
  });

  it("is null for anything that is not a patch with lines", () => {
    for (const not of [null, undefined, "Error: no", {}, { structuredPatch: [] }, { structuredPatch: "x" }, { structuredPatch: [{ oldStart: 1, newStart: 1, lines: [] }] }, { structuredPatch: [null, 4] }]) {
      expect(fromPatch(not)).toBeNull();
    }
    // Numbers that are not numbers leave the lines unnumbered and still shown.
    expect(shape(fromPatch({ structuredPatch: [{ oldStart: "1", lines: ["+a", 7, " b"] }] })!.hunks[0])).toEqual(["+a", " b"]);
  });
});

describe("fromContent", () => {
  it("shows a new file as added lines from 1, without an empty last line", () => {
    const d = fromContent("# Title\n\nbody\n");
    expect(shape(d.hunks[0])).toEqual(["1+# Title", "2+", "3+body"]);
    expect([d.added, d.removed]).toEqual([3, 0]);
    expect(shape(fromContent("").hunks[0])).toEqual(["1+"]);
  });
});

describe("fromEdit", () => {
  it("marks only the lines that differ", () => {
    const d = fromEdit("a\nb\nc\nd", "a\nB\nc\nd\ne");
    expect(shape(d.hunks[0])).toEqual([" a", "-b", "+B", " c", " d", "+e"]);
    expect([d.added, d.removed]).toEqual([2, 1]);
  });

  it("handles a pure addition, a pure removal and no change", () => {
    expect(shape(fromEdit("x", "x\ny").hunks[0])).toEqual([" x", "+y"]);
    expect(shape(fromEdit("x\ny", "y").hunks[0])).toEqual(["-x", " y"]);
    expect(shape(fromEdit("same", "same").hunks[0])).toEqual([" same"]);
    expect(shape(fromEdit("one", "two").hunks[0])).toEqual(["-one", "+two"]);
  });

  it("gives up comparing two huge texts and shows one after the other", () => {
    const a = Array.from({ length: 600 }, (_, i) => `a${i}`).join("\n");
    const b = Array.from({ length: 600 }, (_, i) => `b${i}`).join("\n");
    const d = fromEdit(a, b);
    expect([d.removed, d.added]).toEqual([600, 600]);
    expect(d.hunks[0][0]).toEqual({ sign: "-", text: "a0", n: null });
    expect(d.hunks[0][600]).toEqual({ sign: "+", text: "b0", n: null });
  });
});

describe("diffOf", () => {
  const edit = { file_path: "a.ts", old_string: "x = 1", new_string: "x = 2" };
  const patch = { structuredPatch: [{ oldStart: 7, newStart: 7, lines: ["-x = 1", "+x = 2"] }] };

  it("shows nothing until the tool has run", () => {
    expect(diffOf("Edit", edit, undefined)).toBeNull();
  });

  it("uses what Claude Code reported when the edit went through", () => {
    expect(shape(diffOf("Edit", edit, { isError: false, patch })!.hunks[0])).toEqual(["7-x = 1", "7+x = 2"]);
  });

  it("shows what was asked for when the edit failed, without line numbers", () => {
    expect(shape(diffOf("Edit", edit, { isError: true })!.hunks[0])).toEqual(["-x = 1", "+x = 2"]);
    const multi = { edits: [edit, { old_string: "y", new_string: "z" }, { old_string: 5 }] };
    expect(diffOf("MultiEdit", multi, { isError: true })!.hunks.map(shape)).toEqual([
      ["-x = 1", "+x = 2"],
      ["-y", "+z"],
    ]);
  });

  it("shows a newly written file whole, and an overwritten one only by its patch", () => {
    const write = { file_path: "n.md", content: "one\ntwo" };
    expect(shape(diffOf("Write", write, { isError: false, patch: { type: "create", structuredPatch: [] } })!.hunks[0])).toEqual(["1+one", "2+two"]);
    expect(shape(diffOf("Write", write, { isError: false })!.hunks[0])).toEqual(["1+one", "2+two"]);
    // Written over with nothing reported as changed: showing the whole file as new would be wrong.
    expect(diffOf("Write", write, { isError: false, patch: { type: "update", structuredPatch: [] } })).toBeNull();
    expect(shape(diffOf("Write", write, { isError: false, patch: { type: "update", ...patch } })!.hunks[0])).toEqual(["7-x = 1", "7+x = 2"]);
  });

  it("is null for tools that change no file, whatever they carry", () => {
    expect(diffOf("Bash", { command: "ls" }, { isError: false })).toBeNull();
    expect(diffOf("Read", { file_path: "a", content: "x" }, { isError: false })).toBeNull();
    expect(diffOf("Edit", null, { isError: true })).toBeNull();
    expect(diffOf("Edit", { old_string: 1, new_string: 2 }, { isError: true })).toBeNull();
  });
});
