// Turns a session's event log into rows to render.

import type { SessionEvent } from "./api";
import { visible } from "./safe";

export type Row =
  | { kind: "user"; key: string; text: string; ts: number }
  | { kind: "text"; key: string; text: string }
  | { kind: "thinking"; key: string; text: string }
  | { kind: "tool"; key: string; id: string; name: string; input: any; result?: { text: string; isError: boolean } }
  | { kind: "permission"; key: string; requestId: string; tool: string; input: any; decision: string | null; description?: string | null; superseded?: boolean }
  | { kind: "result"; key: string; cost: number | null; durationMs: number | null; turns: number | null; isError: boolean; text: string | null }
  | { kind: "error"; key: string; message: string };

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
 * What can still change about a row after it first appears: a tool's result and a request's
 * outcome. Everything else about a row is fixed once written, so a row whose state is the same
 * as before does not need drawing again.
 */
export function rowState(r: Row): string {
  if (r.kind === "tool") return r.result ? `done ${r.result.isError} ${r.result.text.length}` : "running";
  if (r.kind === "permission") return `${r.decision} ${r.superseded === true}`;
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

export function toRows(events: SessionEvent[]): Row[] {
  const rows: Row[] = [];
  const tools = new Map<string, Extract<Row, { kind: "tool" }>>();
  const perms = new Map<string, Extract<Row, { kind: "permission" }>>();
  for (const e of events) {
    const key = String(e.seq);
    switch (e.kind) {
      case "user":
        rows.push({ kind: "user", key, text: e.text, ts: e.ts });
        break;
      case "claude": {
        const m = e.msg;
        if (m.type === "assistant") {
          if (m.parent_tool_use_id) break; // inside a subagent; its tool row stands for it
          const content = Array.isArray(m.message?.content) ? m.message.content : [];
          content.forEach((b: any, i: number) => {
            const k = `${key}.${i}`;
            if (!b || typeof b !== "object") return;
            if (b.type === "text" && typeof b.text === "string" && b.text.trim()) rows.push({ kind: "text", key: k, text: b.text });
            else if (b.type === "thinking" && typeof b.thinking === "string" && b.thinking.trim()) rows.push({ kind: "thinking", key: k, text: b.thinking });
            else if (b.type === "tool_use") {
              const row = { kind: "tool" as const, key: k, id: String(b.id), name: String(b.name), input: b.input };
              tools.set(b.id, row);
              rows.push(row);
            }
          });
        } else if (m.type === "user") {
          const content = m.message?.content;
          if (Array.isArray(content)) {
            for (const b of content) {
              if (b?.type === "tool_result" && tools.has(b.tool_use_id)) {
                tools.get(b.tool_use_id)!.result = { text: resultText(b.content), isError: !!b.is_error };
              }
            }
          }
        } else if (m.type === "result") {
          rows.push({
            kind: "result",
            key,
            // The CLI reports a running total; hangar adds what this turn cost.
            cost: m.turn_cost_usd ?? null,
            durationMs: m.duration_ms ?? null,
            turns: m.num_turns ?? null,
            isError: !!m.is_error,
            text: m.is_error ? m.result ?? null : null,
          });
        }
        break;
      }
      case "permission": {
        const row = { kind: "permission" as const, key, requestId: e.request_id, tool: String(e.tool_name), input: e.input, decision: null, description: e.description };
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
        if (p) p.decision = e.behavior;
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
    default: {
      const first = Object.values(input).find((v) => typeof v === "string") as string | undefined;
      return first ?? "";
    }
  }
}
