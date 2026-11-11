import { describe, expect, it } from "vitest";
import type { Session, Status } from "./api";
import { groups, matches, needsYou, projects, samePath, step, visible } from "./views";

let n = 0;
const s = (status: Status, over: Partial<Session> = {}): Session => ({
  id: `s${++n}`,
  rev: 1,
  title: `session ${n}`,
  cwd: "C:\\code\\app",
  created_ms: 0,
  updated_ms: n,
  model: null,
  permission_mode: "default",
  claude_session_id: null,
  status,
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

describe("visible", () => {
  it("orders by group, then most recent first", () => {
    const old = s("idle", { updated_ms: 1 });
    const recent = s("idle", { updated_ms: 50 });
    const asking = s("needs_you", { updated_ms: 2 });
    const working = s("working", { updated_ms: 3 });
    const order = visible([old, working, recent, asking], { kind: "all", project: null }).map((x) => x.id);
    expect(order).toEqual([asking.id, working.id, recent.id, old.id]);
  });

  it("the inbox shows only sessions that need you", () => {
    const list = visible([s("idle"), s("needs_you"), s("error"), s("working")], { kind: "inbox" });
    expect(list.map((x) => x.status).sort()).toEqual(["error", "needs_you"]);
    expect(list.every(needsYou)).toBe(true);
  });

  it("a project view matches folders however they are spelled", () => {
    const a = s("idle", { cwd: "C:\\code\\app" });
    const b = s("idle", { cwd: "c:/code/app/" });
    const other = s("idle", { cwd: "C:\\code\\other" });
    expect(visible([a, b, other], { kind: "all", project: "C:/Code/App" }).map((x) => x.id).sort()).toEqual([a.id, b.id].sort());
  });
});

describe("groups", () => {
  it("splits the sorted list into status groups", () => {
    const list = visible([s("idle"), s("working"), s("starting"), s("hibernated"), s("stopped")], { kind: "all", project: null });
    expect(groups(list).map(([g, items]) => [g, items.length])).toEqual([
      ["Working", 2],
      ["Ready", 1],
      ["Paused", 2],
    ]);
  });
});

describe("projects", () => {
  it("counts sessions per folder, most recently active first", () => {
    const ps = projects([s("idle", { cwd: "C:\\a", updated_ms: 5 }), s("idle", { cwd: "c:/a/", updated_ms: 9 }), s("idle", { cwd: "C:\\b", updated_ms: 7 })]);
    expect(ps.map((p) => [p.name, p.count])).toEqual([
      ["a", 2],
      ["b", 1],
    ]);
    expect(samePath(ps[0].path, "C:/A")).toBe(true);
  });
});

describe("matches", () => {
  it("needs every word somewhere, in any case", () => {
    expect(matches("fix LOGIN", "Fix the login test", "C:/code/app")).toBe(true);
    expect(matches("login app", "Fix the login test", "C:/code/app")).toBe(true);
    expect(matches("logout", "Fix the login test")).toBe(false);
    expect(matches("", "anything")).toBe(true);
  });
});

describe("step", () => {
  it("moves through the list without wrapping", () => {
    const list = [s("idle"), s("idle"), s("idle")];
    expect(step(list, list[0].id, 1)).toBe(list[1].id);
    expect(step(list, list[2].id, 1)).toBe(list[2].id);
    expect(step(list, list[0].id, -1)).toBe(list[0].id);
    expect(step(list, null, 1)).toBe(list[0].id);
    expect(step(list, null, -1)).toBe(list[2].id);
    expect(step([], null, 1)).toBeNull();
  });
});
