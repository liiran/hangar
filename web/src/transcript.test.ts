import { describe, expect, it } from "vitest";
import type { SessionEvent } from "./api";
import { relative, toRows, toolSummary } from "./transcript";

let seq = 0;
const ev = (body: Record<string, unknown>): SessionEvent => ({ seq: ++seq, ts: 1000 + seq, ...body }) as SessionEvent;
const assistant = (content: unknown[], extra: Record<string, unknown> = {}) => ev({ kind: "claude", msg: { type: "assistant", message: { content }, ...extra } });
const toolResults = (content: unknown[]) => ev({ kind: "claude", msg: { type: "user", message: { content } } });

describe("toRows", () => {
  it("renders a full turn in order and attaches tool results to their calls", () => {
    const rows = toRows([
      ev({ kind: "user", text: "list files" }),
      assistant([
        { type: "thinking", thinking: "I should list" },
        { type: "text", text: "Sure." },
        { type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } },
      ]),
      toolResults([{ type: "tool_result", tool_use_id: "t1", content: [{ type: "text", text: "a.txt" }] }]),
      ev({ kind: "claude", msg: { type: "result", total_cost_usd: 0.01, duration_ms: 1200, num_turns: 2, is_error: false } }),
    ]);
    expect(rows.map((r) => r.kind)).toEqual(["user", "thinking", "text", "tool", "result"]);
    const tool = rows[3];
    expect(tool.kind === "tool" && tool.result).toEqual({ text: "a.txt", isError: false });
    const result = rows[4];
    expect(result.kind === "result" && [result.cost, result.durationMs, result.isError]).toEqual([0.01, 1200, false]);
  });

  it("marks failed tool results and stringifies plain content", () => {
    const rows = toRows([
      assistant([{ type: "tool_use", id: "t2", name: "Read", input: { file_path: "x" } }]),
      toolResults([{ type: "tool_result", tool_use_id: "t2", content: "no such file", is_error: true }]),
    ]);
    expect(rows[0].kind === "tool" && rows[0].result).toEqual({ text: "no such file", isError: true });
  });

  it("leaves a tool without a result as still running", () => {
    const rows = toRows([assistant([{ type: "tool_use", id: "t3", name: "Grep", input: { pattern: "x" } }])]);
    expect(rows[0].kind === "tool" && rows[0].result).toBeUndefined();
  });

  it("hides messages from inside subagents and empty text", () => {
    const rows = toRows([assistant([{ type: "text", text: "inner" }], { parent_tool_use_id: "task1" }), assistant([{ type: "text", text: "   " }])]);
    expect(rows).toEqual([]);
  });

  it("records the decision on a permission request", () => {
    const rows = toRows([
      ev({ kind: "permission", request_id: "r1", tool_name: "Write", input: { file_path: "a" } }),
      ev({ kind: "decision", request_id: "r1", behavior: "allow_always" }),
      ev({ kind: "permission", request_id: "r2", tool_name: "Bash", input: { command: "rm x" } }),
    ]);
    expect(rows.map((r) => (r.kind === "permission" ? r.decision : null))).toEqual(["allow_always", null]);
  });

  it("shows errors and failed results with their text", () => {
    const rows = toRows([ev({ kind: "error", message: "claude exited with code 5" }), ev({ kind: "claude", msg: { type: "result", is_error: true, result: "rate limited" } })]);
    expect(rows[0]).toMatchObject({ kind: "error", message: "claude exited with code 5" });
    expect(rows[1]).toMatchObject({ kind: "result", isError: true, text: "rate limited" });
  });

  it("ignores status events and unknown Claude message types", () => {
    expect(toRows([ev({ kind: "status", status: "working" }), ev({ kind: "claude", msg: { type: "system", subtype: "init" } })])).toEqual([]);
  });
});

describe("toolSummary", () => {
  const cwd = "C:\\code\\app";
  it("summarizes common tools", () => {
    expect(toolSummary("Bash", { command: "npm test" })).toBe("npm test");
    expect(toolSummary("Read", { file_path: "C:\\code\\app\\src\\main.rs" }, cwd)).toBe("src/main.rs");
    expect(toolSummary("Grep", { pattern: "TODO", path: "C:/code/app/src" }, cwd)).toBe("TODO  in  src");
    expect(toolSummary("TodoWrite", { todos: [1, 2, 3] })).toBe("3 items");
    expect(toolSummary("mcp__x__y", { query: "q", n: 1 })).toBe("q");
    expect(toolSummary("Bash", null)).toBe("");
  });
});

describe("relative", () => {
  it("shortens paths inside the session folder only", () => {
    expect(relative("C:\\code\\app\\a\\b.txt", "C:\\code\\app")).toBe("a/b.txt");
    expect(relative("c:/CODE/app", "C:\\code\\app\\")).toBe(".");
    expect(relative("C:\\code\\application\\x", "C:\\code\\app")).toBe("C:\\code\\application\\x");
    expect(relative("/etc/hosts")).toBe("/etc/hosts");
  });
});
