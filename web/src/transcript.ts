// Turns a session's event log into rows to render.

import type { DecisionDetail, RewindRestored, SessionEvent } from "./api";
import { ASK_TOOL, PLAN_TOOL } from "./ask";
import { visible } from "./safe";

export type Row =
  | {
      kind: "user";
      key: string;
      text: string;
      ts: number;
      /** Its number in the session's history: what a rewind to it is asked for by. */
      seq: number;
      /** It went to Claude with an id, so Claude Code can go back to before it. */
      canRewind: boolean;
      /** Sent while a turn ran: Claude Code kept no file checkpoint for it. */
      midTurn?: boolean;
    }
  /** The user went back to before a message: what was restored, and the message's text. */
  | { kind: "rewind"; key: string; text: string; code?: RewindRestored; conversation: boolean }
  | { kind: "text"; key: string; text: string }
  | { kind: "thinking"; key: string; text: string }
  | { kind: "tool"; key: string; id: string; name: string; input: any; result?: ToolResult; agent?: AgentRun }
  | {
      kind: "permission";
      key: string;
      requestId: string;
      tool: string;
      input: any;
      decision: string | null;
      detail?: DecisionDetail | null;
      description?: string | null;
      superseded?: boolean;
      /** The helper agent that asked, when it was not Claude itself. */
      agentId?: string | null;
    }
  | { kind: "result"; key: string; cost: number | null; durationMs: number | null; turns: number | null; isError: boolean; stopped: boolean; text: string | null }
  | { kind: "error"; key: string; message: string };

type ToolRow = Extract<Row, { kind: "tool" }>;

/** The tools with which Claude starts a helper agent ("Task" is the older name). */
export const AGENT_TOOLS = new Set(["Agent", "Task"]);

/** What a helper agent did, kept with the Agent call that started it. */
export interface AgentRun {
  /** Claude Code's id for the agent, once it has said it. */
  id?: string;
  /** It runs beside the conversation instead of inside Claude's turn. */
  background: boolean;
  /** The agent's own tool calls and words, in order. */
  steps: Row[];
  /** Claude Code's own count of what it has used so far, or used in all. */
  toolUses?: number;
  tokens?: number;
  durationMs?: number;
  /** How it ended, once Claude Code has said so. */
  ended?: "completed" | "failed" | "stopped";
  /** Goes up with every change to the agent or to one of its steps. */
  rev: number;
}

export interface ToolResult {
  text: string;
  isError: boolean;
  /** For an edit: the changed lines as Claude Code reported them (see diff.ts). */
  patch?: unknown;
}

/**
 * Claude Code's own "you are signed out" reply. It says to run `/login`, a command of Claude
 * Code's terminal. hangar has no such command, so the app says where to sign in.
 */
export function needsSignIn(text: string): boolean {
  return text.length < 200 && /\/login\b/.test(text) && /not logged in|invalid api key|please run/i.test(text);
}

/** How many of a session's newest rows are drawn when it opens, and how many more "Show more" adds. */
export const PAGE = 300;

/**
 * Where drawing starts, given where it started when the session opened (`opened`), how many
 * rows there are now and how many earlier ones were asked for. It never moves down while rows
 * are only added, so nothing on screen goes away while you read. A rewind takes rows away: then
 * it moves down to the newest page, so that what is left is drawn and not an empty feed.
 */
export function drawFrom(opened: number, rows: number, earlier: number): { opened: number; from: number } {
  const start = opened > Math.max(0, rows - PAGE) ? Math.max(0, rows - PAGE) : opened;
  return { opened: start, from: Math.max(0, start - earlier) };
}

/**
 * What can still change about a row after it first appears: a tool's result and a request's
 * outcome. Everything else about a row is fixed once written, so a row whose state is the same
 * as before does not need drawing again.
 */
export function rowState(r: Row): string {
  if (r.kind === "tool") {
    const own = r.result ? `done ${r.result.isError} ${r.result.text.length} ${r.result.patch != null}` : "running";
    return r.agent ? `${own} agent ${r.agent.rev}` : own;
  }
  if (r.kind === "permission") return `${r.decision} ${r.superseded === true} ${r.detail != null}`;
  return "";
}

function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b: any) => (b?.type === "text" ? b.text : b?.type === "image" ? "[image]" : ""))
      .filter(Boolean)
      .join("\n");
  }
  return content == null ? "" : JSON.stringify(content, null, 2);
}

/** One line of what a helper agent is doing: a step, or a run of reads and searches as one line. */
export type AgentLine =
  | { kind: "step"; row: ToolRow }
  | { kind: "text"; key: string; text: string }
  | { kind: "summary"; key: string; searches: number; reads: number; active: boolean };

const SEARCHES = new Set(["Grep", "Glob"]);

/** "Searching for 2 patterns, reading 3 files…" while it goes on, "Searched for …, read …" after: Claude Code's wording. */
export function readSearchSummary(searches: number, reads: number, active: boolean): string {
  const parts: string[] = [];
  if (searches > 0) parts.push(`${active ? "Searching for" : "Searched for"} ${searches} ${searches === 1 ? "pattern" : "patterns"}`);
  if (reads > 0) parts.push(`${active ? (parts.length === 0 ? "Reading" : "reading") : parts.length === 0 ? "Read" : "read"} ${reads} ${reads === 1 ? "file" : "files"}`);
  const text = parts.join(", ");
  return active ? `${text}…` : text;
}

/** How many of an agent's lines show while it runs (Claude Code's terminal shows the same number). */
export const AGENT_LINES = 3;

/**
 * What an agent has done, as the lines to show while it runs. This is how Claude Code's
 * terminal builds them (read from its code, 2.1.329):
 * - reads and searches are not lines of their own. They are counted, once each has finished,
 *   into one line that stands for the whole run of them: "Searching for 1 pattern, reading 2
 *   files…" while the run is the newest thing, "Searched for 1 pattern, read 2 files" after;
 * - every other tool call is a line, and so is what the agent says;
 * - the newest `AGENT_LINES` show. `hidden` is how many lines above them stand for tool calls.
 */
export function agentLines(agent: AgentRun, running: boolean): { lines: AgentLine[]; hidden: number } {
  const all: AgentLine[] = [];
  let run: { key: string; searches: number; reads: number } | null = null;
  const close = (active: boolean) => {
    if (run && run.searches + run.reads > 0) all.push({ kind: "summary", ...run, active });
    run = null;
  };
  for (const s of agent.steps) {
    if (s.kind === "tool" && (s.name === "Read" || SEARCHES.has(s.name))) {
      run ??= { key: s.key, searches: 0, reads: 0 };
      if (s.result) run[SEARCHES.has(s.name) ? "searches" : "reads"]++;
    } else if (s.kind === "tool") {
      close(false);
      all.push({ kind: "step", row: s });
    } else if (s.kind === "text") {
      close(false);
      all.push({ kind: "text", key: s.key, text: s.text });
    }
  }
  close(running);
  const shown = all.slice(-AGENT_LINES);
  return { lines: shown, hidden: all.slice(0, all.length - shown.length).filter((l) => l.kind !== "text").length };
}

/** "2 tool uses · 20.4k tokens": what an agent has used, in Claude Code's words. Empty when nothing is known. */
export function agentUsage(agent: AgentRun | undefined, withTime: boolean): string {
  const uses = agent?.toolUses ?? agent?.steps.filter((s) => s.kind === "tool").length ?? 0;
  const tokens = agent?.tokens ?? 0;
  const ms = agent?.durationMs ?? 0;
  const k = (n: number) => (n < 1000 ? String(n) : `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`);
  const seconds = Math.round(ms / 1000);
  const time = seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
  return [uses > 0 && `${uses} ${uses === 1 ? "tool use" : "tool uses"}`, tokens > 0 && `${k(tokens)} tokens`, withTime && ms >= 1000 && time].filter(Boolean).join(" · ");
}

export function toRows(events: SessionEvent[]): Row[] {
  const rows: Row[] = [];
  const tools = new Map<string, ToolRow>();
  const perms = new Map<string, Extract<Row, { kind: "permission" }>>();
  // The user's messages by their number, for the row that says which one was gone back to.
  const prompts = new Map<number, string>();
  const byUuid = new Map<string, Extract<Row, { kind: "user" }>>();
  // A helper agent's steps are kept with the Agent call that started it.
  const owner = new Map<string, ToolRow>();
  const byTask = new Map<string, ToolRow>();
  const agentOf = (row: ToolRow) => (row.agent ??= { background: false, steps: [], rev: 0 });
  /** Marks an agent, and every agent it runs inside, as changed. */
  const changed = (row: ToolRow | undefined) => {
    for (let r = row; r; r = owner.get(r.id)) if (r.agent) r.agent.rev++;
  };
  for (const e of events) {
    const key = String(e.seq);
    switch (e.kind) {
      case "user": {
        const row = { kind: "user" as const, key, text: e.text, ts: e.ts, seq: e.seq, canRewind: typeof e.uuid === "string", midTurn: e.mid_turn === true };
        rows.push(row);
        prompts.set(e.seq, e.text);
        if (typeof e.uuid === "string") byUuid.set(e.uuid, row);
        break;
      }
      case "unsent":
        // Messages that never reached Claude: the session was stopped first.
        for (const u of e.uuids ?? []) {
          const row = byUuid.get(u);
          if (row) row.canRewind = false;
        }
        break;
      case "rewind": {
        // The conversation went back: the message and all after it are gone, as in Claude
        // Code's terminal, up to what was sent while the rewind ran (`until`). Every row's
        // key starts with the number of the event it came from.
        if (e.conversation) {
          const kept = rows.filter((r) => {
            const n = Number(r.key.split(".")[0]);
            return n < e.to || (e.until != null && n >= e.until);
          });
          rows.length = 0;
          rows.push(...kept);
        }
        rows.push({ kind: "rewind", key, text: prompts.get(e.to) ?? "", code: e.code ?? undefined, conversation: !!e.conversation });
        break;
      }
      case "claude": {
        const m = e.msg;
        if (m.type === "assistant") {
          // A message from inside a helper agent belongs to the Agent call that started it.
          const inside = m.parent_tool_use_id ? tools.get(m.parent_tool_use_id) : undefined;
          if (m.parent_tool_use_id && !inside) break;
          const into = inside ? agentOf(inside).steps : rows;
          const content = Array.isArray(m.message?.content) ? m.message.content : [];
          content.forEach((b: any, i: number) => {
            const k = `${key}.${i}`;
            if (!b || typeof b !== "object") return;
            if (b.type === "text" && typeof b.text === "string" && b.text.trim()) into.push({ kind: "text", key: k, text: b.text });
            else if (b.type === "thinking" && typeof b.thinking === "string" && b.thinking.trim() && !inside) into.push({ kind: "thinking", key: k, text: b.thinking });
            else if (b.type === "tool_use") {
              const row = { kind: "tool" as const, key: k, id: String(b.id), name: String(b.name), input: b.input };
              tools.set(row.id, row);
              if (inside) owner.set(row.id, inside);
              into.push(row);
            }
          });
          changed(inside);
        } else if (m.type === "user") {
          const content = m.message?.content;
          if (Array.isArray(content)) {
            for (const b of content) {
              const row = b?.type === "tool_result" ? tools.get(b.tool_use_id) : undefined;
              if (!row) continue;
              const full = m.tool_use_result && typeof m.tool_use_result === "object" ? m.tool_use_result : undefined;
              const text = resultText(b.content);
              if (AGENT_TOOLS.has(row.name)) {
                // What the Agent tool reports about the helper: whether it went to the background,
                // and for one that ran to its end, Claude Code's own totals.
                row.result = { text, isError: !!b.is_error };
                const agent = agentOf(row);
                if (typeof full?.agentId === "string") agent.id = full.agentId;
                if (full?.status === "async_launched" || text.startsWith("Async agent launched")) agent.background = true;
                if (full?.status === "completed") {
                  agent.ended ??= "completed";
                  if (typeof full.totalToolUseCount === "number") agent.toolUses = full.totalToolUseCount;
                  if (typeof full.totalTokens === "number") agent.tokens = full.totalTokens;
                  if (typeof full.totalDurationMs === "number") agent.durationMs = full.totalDurationMs;
                }
                changed(row);
              } else {
                row.result = { text, isError: !!b.is_error, patch: full };
                changed(owner.get(row.id));
              }
            }
          }
        } else if (m.type === "system" && typeof m.subtype === "string" && m.subtype.startsWith("task_")) {
          // Claude Code's own account of a helper agent: started, what it has used, ended.
          const row = (typeof m.tool_use_id === "string" ? tools.get(m.tool_use_id) : undefined) ?? byTask.get(String(m.task_id));
          if (!row) break;
          const agent = agentOf(row);
          if (m.subtype === "task_started") {
            agent.id = String(m.task_id);
            agent.background = m.is_backgrounded === true;
            byTask.set(agent.id, row);
          }
          if (m.usage && typeof m.usage === "object") {
            if (typeof m.usage.tool_uses === "number") agent.toolUses = m.usage.tool_uses;
            if (typeof m.usage.total_tokens === "number") agent.tokens = m.usage.total_tokens;
            if (typeof m.usage.duration_ms === "number") agent.durationMs = m.usage.duration_ms;
          }
          if ((m.subtype === "task_updated" || m.subtype === "task_notification") && typeof m.status === "string" && m.status !== "running" && m.status !== "pending") {
            agent.ended = m.status === "completed" || m.status === "failed" ? m.status : "stopped";
          }
          changed(row);
        } else if (m.type === "result") {
          rows.push({
            kind: "result",
            key,
            // The CLI reports a running total; hangar adds what this turn cost.
            cost: m.turn_cost_usd ?? null,
            durationMs: m.duration_ms ?? null,
            turns: m.num_turns ?? null,
            isError: !!m.is_error,
            // A turn the user stopped ends as an "error" too; it did not fail.
            stopped: !!m.is_error && typeof m.terminal_reason === "string" && m.terminal_reason.startsWith("aborted"),
            text: m.is_error ? m.result ?? null : null,
          });
        }
        break;
      }
      case "permission": {
        const row = { kind: "permission" as const, key, requestId: e.request_id, tool: String(e.tool_name), input: e.input, decision: null, description: e.description, agentId: e.agent_id };
        // If an id is ever used again, the older unanswered request must not
        // come back to life showing its old content.
        const earlier = perms.get(e.request_id);
        if (earlier && earlier.decision == null) earlier.superseded = true;
        perms.set(e.request_id, row);
        rows.push(row);
        break;
      }
      case "decision": {
        const p = perms.get(e.request_id);
        if (p) {
          p.decision = e.behavior;
          p.detail = e.detail ?? null;
        }
        break;
      }
      case "error":
        rows.push({ kind: "error", key, message: e.message });
        break;
    }
  }
  return rows;
}

/** Shows `path` relative to `cwd` when it lies inside it. */
export function relative(path: string, cwd?: string): string {
  if (!cwd) return path;
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const p = norm(path);
  const c = norm(cwd);
  if (p.toLowerCase() === c.toLowerCase()) return ".";
  if (p.toLowerCase().startsWith(c.toLowerCase() + "/")) return p.slice(c.length + 1);
  return path;
}

/** A one-line summary of a tool call's input, with invisible characters written out. */
export function toolSummary(name: string, input: any, cwd?: string): string {
  return visible(summarize(name, input, cwd));
}

function summarize(name: string, input: any, cwd?: string): string {
  if (!input || typeof input !== "object") return "";
  const pick = (...keys: string[]) => {
    const v = keys.map((k) => input[k]).find((v) => typeof v === "string" && v.length > 0) as string | undefined;
    return v && /path|^file/.test(keys.join()) ? relative(v, cwd) : v;
  };
  switch (name) {
    case "Bash":
      return pick("command") ?? "";
    case "Read":
    case "Write":
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return pick("file_path", "notebook_path") ?? "";
    case "Glob":
    case "Grep":
      return [pick("pattern"), typeof input.path === "string" && input.path ? relative(input.path, cwd) : null].filter(Boolean).join("  in  ");
    case "WebFetch":
      return pick("url") ?? "";
    case "WebSearch":
      return pick("query") ?? "";
    case "Task":
    case "Agent":
      return pick("description", "prompt") ?? "";
    case "TodoWrite":
      return Array.isArray(input.todos) ? `${input.todos.length} items` : "";
    case ASK_TOOL: {
      const asked = Array.isArray(input.questions) ? input.questions.filter((q: any) => typeof q?.question === "string") : [];
      return asked.length > 0 ? `${asked[0].question}${asked.length > 1 ? `  +${asked.length - 1} more` : ""}` : "";
    }
    case PLAN_TOOL: {
      // The plan's title: its first line of text, without the heading marks.
      const first = typeof input.plan === "string" ? input.plan.split("\n").find((l: string) => l.trim()) : undefined;
      return first ? first.replace(/^#+\s*/, "").trim() : "";
    }
    default: {
      const first = Object.values(input).find((v) => typeof v === "string") as string | undefined;
      return first ?? "";
    }
  }
}
