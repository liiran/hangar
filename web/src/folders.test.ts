import { describe, expect, it } from "vitest";
import type { Folders } from "./api";
import { into, isPathLike, lastSegment, sections, tidy } from "./folders";

const listing = (over: Partial<Folders>): Folders => ({ dir: "C:\\code", exists: true, parent: "C:\\", entries: [], truncated: false, ...over });

describe("isPathLike", () => {
  it("accepts absolute and home paths only", () => {
    for (const p of ["C:\\code", "c:/code", "C:", "\\\\server\\share", "/home/me", "~", "~/code", "~\\code"]) expect(isPathLike(p), p).toBe(true);
    for (const p of ["", "code", "hangar app", "..\\x", "./x"]) expect(isPathLike(p), p).toBe(false);
  });
});

describe("path helpers", () => {
  it("tidies trailing separators but keeps roots", () => {
    expect(tidy("C:\\code\\app\\")).toBe("C:\\code\\app");
    expect(tidy("C:")).toBe("C:\\");
    expect(tidy("c:/")).toBe("c:\\");
    expect(tidy("/")).toBe("/");
    expect(lastSegment("C:\\code\\ap")).toBe("ap");
    expect(into("C:\\code", "\\")).toBe("C:\\code\\");
    expect(into("C:\\", "\\")).toBe("C:\\");
  });
});

describe("sections", () => {
  const entries = [
    { name: "api", path: "C:\\code\\api" },
    { name: "web", path: "C:\\code\\web" },
  ];

  it("an existing folder can be used, left for its parent, or entered", () => {
    const s = sections("C:\\code\\", listing({ dir: "C:\\code\\", entries }), []);
    expect(s.top).toEqual([{ kind: "use", path: "C:\\code" }]);
    expect(s.folders.map((f) => f.kind)).toEqual(["up", "folder", "folder"]);
    expect(s.heading).toBe("In code");
  });

  it("a new name offers to create the folder", () => {
    const s = sections("C:\\code\\todo-app", listing({ exists: false, entries: [] }), []);
    expect(s.top).toEqual([{ kind: "create", path: "C:\\code\\todo-app", name: "todo-app" }]);
  });

  it("does not offer to create a folder that is already listed", () => {
    const s = sections("C:\\code\\API", listing({ exists: false, entries: [entries[0]] }), []);
    expect(s.top).toEqual([]);
  });

  it("words search recent folders and never list a relative path", () => {
    const recent = ["C:\\code\\personal\\hangar", "C:\\code\\work\\ifr", "D:\\games"];
    const s = sections("hangar", listing({ entries }), recent);
    expect(s.recent).toEqual([{ kind: "recent", path: recent[0] }]);
    expect(s.folders).toEqual([]);
    expect(sections("", listing({ dir: null, exists: false, parent: null, entries }), recent).recent).toHaveLength(3);
  });

  it("the top level lists places", () => {
    const s = sections("", listing({ dir: null, exists: false, parent: null, entries: [{ name: "Home", path: "C:\\Users\\me" }] }), []);
    expect(s.heading).toBe("Places");
    expect(s.top).toEqual([]);
    expect(s.folders).toEqual([{ kind: "folder", path: "C:\\Users\\me", name: "Home" }]);
  });
});
