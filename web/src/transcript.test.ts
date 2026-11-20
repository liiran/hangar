import { describe, expect, it } from "vitest";
import type { SessionEvent } from "./api";
import { agentLines, agentUsage, needsSignIn, readSearchSummary, relative, rowState, toRows, toolSummary } from "./transcript";

let seq = 0;
const ev = (body: Record<string, unknown>): SessionEvent => ({ seq: ++seq, ts: 1000 + seq, ...body }) as SessionEvent;
const assistant = (content: unknown[], extra: Record<string, unknown> = {}) => ev({ kind: "claude", msg: { type: "assistant", message: { content }, ...extra } });
const toolResults = (content: unknown[]) => ev({ kind: "claude", msg: { type: "user", message: { content } } });

describe("needsSignIn", () => {
  it("knows Claude Code's signed-out replies, and nothing else", () => {
    expect(needsSignIn("Not logged in · Please run /login")).toBe(true);
    expect(needsSignIn("Invalid API key · Please run /login")).toBe(true);
    expect(needsSignIn("I added a /login route to the server.")).toBe(false);
    expect(needsSignIn("Please run npm test")).toBe(false);
    expect(needsSignIn("Not logged in. " + "x".repeat(300) + " /login")).toBe(false);
  });
});

describe("rowState", () => {
  it("changes when a tool gets its result or a request its answer, and not otherwise", () => {
    const call = assistant([{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } }]);
    const before = toRows([call]);
    const after = toRows([call, toolResults([{ type: "tool_result", tool_use_id: "t1", content: "a.txt" }])]);
    expect(rowState(before[0])).not.toBe(rowState(after[0]));
    const text = assistant([{ type: "text", text: "hello" }]);
    expect(rowState(toRows([text])[0])).toBe(rowState(toRows([text, call])[0]));
    const ask = ev({ kind: "permission", request_id: "p1", tool_name: "Bash", input: {} });
    const asked = toRows([ask]);
    const answered = toRows([ask, ev({ kind: "decision", request_id: "p1", behavior: "allow" })]);
    expect(rowState(asked[0])).not.toBe(rowState(answered[0]));
  });
});

describe("questions, plans and edits", () => {
  it("keeps what the user chose with the request it answered", () => {
    const rows = toRows([
      ev({ kind: "permission", request_id: "q1", tool_name: "AskUserQuestion", input: { questions: [] } }),
      ev({ kind: "decision", request_id: "q1", behavior: "allow", detail: { answers: { "Which colour?": "Green" } } }),
      ev({ kind: "permission", request_id: "p1", tool_name: "ExitPlanMode", input: { plan: "# Plan" } }),
    ]);
    expect(rows[0]).toMatchObject({ kind: "permission", decision: "allow", detail: { answers: { "Which colour?": "Green" } } });
    expect(rows[1]).toMatchObject({ kind: "permission", decision: null });
    // Events written before answers were kept have no detail.
    const old = toRows([ev({ kind: "permission", request_id: "o", tool_name: "Bash", input: {} }), ev({ kind: "decision", request_id: "o", behavior: "deny" })]);
    expect(old[0]).toMatchObject({ decision: "deny", detail: null });
  });

  it("attaches Claude Code's report of an edit to its tool row, and redraws the row for it", () => {
    const call = assistant([{ type: "tool_use", id: "e1", name: "Edit", input: { file_path: "a.ts", old_string: "a", new_string: "b" } }]);
    const patch = { structuredPatch: [{ oldStart: 3, newStart: 3, lines: ["-a", "+b"] }] };
    const done = ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "e1", content: "updated" }] }, tool_use_result: patch } });
    const row = toRows([call, done])[0];
    expect(row).toMatchObject({ kind: "tool", result: { text: "updated", isError: false, patch } });
    expect(rowState(row)).not.toBe(rowState(toRows([call])[0]));
    // A tool that reports nothing structured (or an error text) has no patch.
    const bash = assistant([{ type: "tool_use", id: "b1", name: "Bash", input: { command: "ls" } }]);
    const plain = ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "b1", content: "x" }] }, tool_use_result: "Error: no" } });
    expect((toRows([bash, plain])[0] as any).result.patch).toBeUndefined();
  });

  it("tells a turn the user stopped from one that failed", () => {
    const result = (extra: Record<string, unknown>) => toRows([ev({ kind: "claude", msg: { type: "result", is_error: true, result: null, ...extra } })])[0];
    expect(result({ subtype: "error_during_execution", terminal_reason: "aborted_streaming" })).toMatchObject({ isError: true, stopped: true });
    expect(result({ subtype: "error_during_execution" })).toMatchObject({ isError: true, stopped: false });
    expect(toRows([ev({ kind: "claude", msg: { type: "result", is_error: false, terminal_reason: "aborted_streaming" } })])[0]).toMatchObject({ stopped: false });
  });

  it("summarizes a question by what it asks and a plan by its title", () => {
    expect(toolSummary("AskUserQuestion", { questions: [{ question: "Which colour?" }, { question: "Which fruit?" }] })).toBe("Which colour?  +1 more");
    expect(toolSummary("AskUserQuestion", { questions: [{ question: "Only one?" }] })).toBe("Only one?");
    expect(toolSummary("AskUserQuestion", { questions: "x" })).toBe("");
    expect(toolSummary("ExitPlanMode", { plan: "\n\n## Plan: split the auth module\n\n1. Move it", planFilePath: "p.md" })).toBe("Plan: split the auth module");
    expect(toolSummary("ExitPlanMode", {})).toBe("");
  });
});

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
      ev({ kind: "claude", msg: { type: "result", total_cost_usd: 0.05, turn_cost_usd: 0.01, duration_ms: 1200, num_turns: 2, is_error: false } }),
    ]);
    expect(rows.map((r) => r.kind)).toEqual(["user", "thinking", "text", "tool", "result"]);
    const tool = rows[3];
    expect(tool.kind === "tool" && tool.result).toEqual({ text: "a.txt", isError: false });
    const result = rows[4];
    expect(result.kind === "result" && [result.cost, result.durationMs, result.isError]).toEqual([0.01, 1200, false]);
  });

  it("shows what the turn cost, not the process's running total", () => {
    const rows = toRows([ev({ kind: "claude", msg: { type: "result", total_cost_usd: 0.3, duration_ms: 5, is_error: false } })]);
    expect(rows[0].kind === "result" && rows[0].cost).toBeNull();
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

  it("hides empty text", () => {
    expect(toRows([assistant([{ type: "text", text: "   " }])])).toEqual([]);
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

describe("helper agents", () => {
  const sys = (subtype: string, fields: Record<string, unknown>) => ev({ kind: "claude", msg: { type: "system", subtype, ...fields } });
  const inside = (content: unknown[]) => assistant(content, { parent_tool_use_id: "a1", agent_id: "task1" });
  const insideResult = (id: string, content: string, extra: Record<string, unknown> = {}) =>
    ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: id, content }] }, parent_tool_use_id: "a1", ...extra } });
  const call = assistant([{ type: "tool_use", id: "a1", name: "Agent", input: { description: "Tidy the notes", prompt: "Tidy them." } }]);
  const started = sys("task_started", { task_id: "task1", tool_use_id: "a1", description: "Tidy the notes", is_backgrounded: true });
  const use = (id: string, name: string, input: Record<string, unknown> = {}) => inside([{ type: "tool_use", id, name, input }]);
  const agentOf = (events: SessionEvent[]) => {
    const rows = toRows(events);
    const row = rows[0];
    if (row.kind !== "tool" || !row.agent) throw new Error("no agent on the first row");
    return { rows, row, agent: row.agent };
  };

  it("keeps an agent's steps and words with the call that started it, not in the feed", () => {
    const { rows, agent } = agentOf([call, started, use("s1", "Read", { file_path: "a.ts" }), insideResult("s1", "one"), inside([{ type: "text", text: "All tidy." }])]);
    expect(rows).toHaveLength(1);
    expect(agent.id).toBe("task1");
    expect(agent.background).toBe(true);
    expect(agent.steps.map((s) => s.kind)).toEqual(["tool", "text"]);
    expect(agent.steps[0].kind === "tool" && agent.steps[0].result?.text).toBe("one");
    // An edit inside an agent keeps its changed lines, like one of Claude's own.
    const edited = agentOf([call, use("s2", "Edit", { file_path: "a.ts" }), insideResult("s2", "ok", { tool_use_result: { hunks: [] } })]);
    expect(edited.agent.steps[0].kind === "tool" && edited.agent.steps[0].result?.patch).toEqual({ hunks: [] });
  });

  it("drops a message from inside an agent whose call is not in the log", () => {
    expect(toRows([assistant([{ type: "text", text: "inner" }], { parent_tool_use_id: "gone" })])).toEqual([]);
  });

  it("takes how much an agent used, and how it ended, from Claude Code", () => {
    const progress = sys("task_progress", { task_id: "task1", tool_use_id: "a1", description: "Reading a.ts", usage: { total_tokens: 20400, tool_uses: 2, duration_ms: 5600 } });
    const running = agentOf([call, started, progress]).agent;
    expect([running.toolUses, running.tokens, running.ended]).toEqual([2, 20400, undefined]);
    expect(agentUsage(running, false)).toBe("2 tool uses · 20.4k tokens");
    // `task_updated` names the task only by its id.
    const done = agentOf([call, started, progress, sys("task_updated", { task_id: "task1", status: "completed" })]).agent;
    expect(done.ended).toBe("completed");
    expect(agentUsage(done, true)).toBe("2 tool uses · 20.4k tokens · 6s");
    expect(agentOf([call, started, sys("task_updated", { task_id: "task1", status: "killed" }), sys("task_notification", { task_id: "task1", tool_use_id: "a1", status: "stopped" })]).agent.ended).toBe("stopped");
    expect(agentOf([call, started, sys("task_notification", { task_id: "task1", tool_use_id: "a1", status: "failed" })]).agent.ended).toBe("failed");
    expect(agentUsage({ background: false, steps: [], rev: 0, toolUses: 1, tokens: 999, durationMs: 83_000 }, true)).toBe("1 tool use · 999 tokens · 1m 23s");
    // Rounded as a whole, so that 119.6 s is not "1m 60s".
    expect(agentUsage({ background: false, steps: [], rev: 0, toolUses: 1, durationMs: 119_600 }, true)).toBe("1 tool use · 2m 0s");
    expect(agentUsage(undefined, true)).toBe("");
  });

  it("reads the Agent tool's own result: the totals of one that ran to its end, the launch of one in the background", () => {
    const result = (full: unknown, content = "[Subagent hand-back] …") =>
      ev({ kind: "claude", msg: { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "a1", content }] }, tool_use_result: full } });
    const fg = agentOf([call, result({ agentId: "task1", status: "completed", totalDurationMs: 4818, totalTokens: 21758, totalToolUseCount: 2 })]);
    expect([fg.agent.id, fg.agent.ended, fg.agent.background]).toEqual(["task1", "completed", false]);
    expect(agentUsage(fg.agent, true)).toBe("2 tool uses · 21.8k tokens · 5s");
    // The totals are not an edit's changed lines.
    expect(fg.row.result?.patch).toBeUndefined();
    const bg = agentOf([call, result({ agentId: "task1", status: "async_launched" }, "Async agent launched successfully.")]).agent;
    expect([bg.background, bg.ended]).toEqual([true, undefined]);
    // A log from before hangar kept these: the launch text alone says it went to the background.
    expect(agentOf([call, result(undefined, "Async agent launched successfully. …")]).agent.background).toBe(true);
  });

  it("draws an Agent row again whenever the agent or one of its steps changes, also an agent inside an agent", () => {
    const base = [call, started, use("s1", "Bash", { command: "npm test" })];
    const state = (events: SessionEvent[]) => rowState(toRows(events)[0]);
    expect(state(base)).not.toBe(state([call, started]));
    expect(state([...base, insideResult("s1", "12 passed")])).not.toBe(state(base));
    expect(state([...base, sys("task_progress", { task_id: "task1", tool_use_id: "a1", usage: { tool_uses: 1 } })])).not.toBe(state(base));
    const nested = [call, use("a2", "Agent", { description: "inner" })];
    const deeper = assistant([{ type: "tool_use", id: "d1", name: "Read", input: {} }], { parent_tool_use_id: "a2" });
    expect(state([...nested, deeper])).not.toBe(state(nested));
    const inner = agentOf([...nested, deeper]).agent.steps[0];
    expect(inner.kind === "tool" && inner.agent?.steps.map((s) => s.kind)).toEqual(["tool"]);
  });

  it("shows a running agent's newest three lines as Claude Code does: reads and searches counted into one line", () => {
    const names = ["Bash", "Read", "Grep", "Read", "Edit", "Glob", "Read"];
    // Each step is a tool call and its result.
    const steps = names.flatMap((name, i) => [use(`s${i}`, name), insideResult(`s${i}`, "ok")]);
    const say = inside([{ type: "text", text: "Halfway there.\nMore below." }]);
    const shown = (events: SessionEvent[], running = true) => {
      const { lines, hidden } = agentLines(agentOf([call, started, ...events]).agent, running);
      return { lines: lines.map((l) => (l.kind === "summary" ? readSearchSummary(l.searches, l.reads, l.active) : l.kind === "text" ? l.text : l.row.name)), hidden };
    };
    const first = (n: number) => steps.slice(0, 2 * n);
    expect(shown([])).toEqual({ lines: [], hidden: 0 });
    // A read that is still going on is not counted yet: nothing shows for it.
    expect(shown([...first(1), steps[2]])).toEqual({ lines: ["Bash"], hidden: 0 });
    // One finished read is already the summary line, not a line of its own.
    expect(shown(first(2))).toEqual({ lines: ["Bash", "Reading 1 file…"], hidden: 0 });
    expect(shown(first(4))).toEqual({ lines: ["Bash", "Searching for 1 pattern, reading 2 files…"], hidden: 0 });
    // Another kind of step ends the run: it is then in the past.
    expect(shown(first(5))).toEqual({ lines: ["Bash", "Searched for 1 pattern, read 2 files", "Edit"], hidden: 0 });
    // Only the newest three lines show. What is counted above them is lines, not the tool calls inside a summary.
    expect(shown(first(7))).toEqual({ lines: ["Searched for 1 pattern, read 2 files", "Edit", "Searching for 1 pattern, reading 1 file…"], hidden: 1 });
    // What the agent says is a line too, ends a run, and is not counted as a tool use.
    expect(shown([...first(4), say, ...steps.slice(8)])).toEqual({ lines: ["Halfway there.\nMore below.", "Edit", "Searching for 1 pattern, reading 1 file…"], hidden: 2 });
    // Once the agent has stopped, nothing is still going on.
    expect(shown(first(7), false).lines[2]).toBe("Searched for 1 pattern, read 1 file");
    expect(readSearchSummary(2, 0, true)).toBe("Searching for 2 patterns…");
    expect(readSearchSummary(0, 3, false)).toBe("Read 3 files");
  });

  it("says which agent asked for permission", () => {
    const rows = toRows([ev({ kind: "permission", request_id: "p9", tool_name: "Edit", input: {}, agent_id: "task1" }), ev({ kind: "permission", request_id: "p10", tool_name: "Edit", input: {} })]);
    expect(rows.map((r) => r.kind === "permission" && r.agentId)).toEqual(["task1", undefined]);
  });
});
