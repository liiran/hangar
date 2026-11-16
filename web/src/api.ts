// Types and calls for the daemon's JSON API (crates/hangar/src/api.rs).

export type Status = "queued" | "starting" | "working" | "needs_you" | "idle" | "hibernated" | "stopped" | "error";

export interface Pending {
  request_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  suggestions: unknown[] | null;
  /** "Always allow" has session rules to remember. */
  can_remember: boolean;
}

export interface Session {
  id: string;
  /** Higher is newer; used to drop stale updates. */
  rev: number;
  title: string;
  cwd: string;
  created_ms: number;
  updated_ms: number;
  model: string | null;
  permission_mode: string;
  claude_session_id: string | null;
  status: Status;
  status_detail: string | null;
  cost_usd: number;
  turns: number;
  preview: string;
  peak_bytes: number;
  pid: number | null;
  memory_bytes: number | null;
  pending: Pending[];
  queue_position: number | null;
  mcp: { name: string; status: string }[];
  /** How full the context window was when Claude last said; absent until it has. */
  context?: ContextUsage | null;
}

/** Claude Code's own figures for its context window (what `/context` shows in a terminal). */
export interface ContextUsage {
  used_tokens: number;
  max_tokens: number;
  /** Claude Code's own whole-number percentage. */
  percent: number;
  model: string | null;
  /** Where Claude Code summarizes the conversation by itself, when it does. */
  auto_compact_at: number | null;
  parts: { name: string; tokens: number }[];
  at_ms: number;
}

/** One of the plan's usage windows, as Claude Code reports it. */
export interface LimitWindow {
  /** `five_hour`, `seven_day`. */
  name: string;
  /** The share used: 0.46 is 46%. */
  used: number;
  /** When it starts over, in seconds since 1970. */
  resets_at: number | null;
  /** `allowed`, `allowed_warning` or `rejected`. */
  status: string;
}

export interface Limits {
  windows: LimitWindow[];
  at_ms: number;
}

export type Decision = "allow" | "allow_always" | "deny" | "cancelled";

/** What the user chose beyond allow or deny. */
export interface DecisionDetail {
  /** A question: the answer to each, by question text. */
  answers?: Record<string, string>;
  /** A plan, approved: the mode the session went on in. */
  mode?: string;
  /** A plan, sent back: what the user asked to change. */
  feedback?: string;
  /** A plan, not approved, with nothing said: the turn was stopped. */
  stopped?: boolean;
  /** A question the user wanted to talk over first. */
  clarify?: boolean;
}

/** What goes with a decision: each field fits one kind of request (the daemon refuses the rest). */
export interface DecisionExtra {
  answers?: Record<string, string>;
  mode?: string;
  feedback?: string;
  stop?: boolean;
  clarify?: boolean;
}

export type EventBody =
  | { kind: "user"; text: string }
  | { kind: "claude"; msg: any }
  | { kind: "permission"; request_id: string; tool_name: string; input: any; suggestions?: unknown; description?: string | null }
  | { kind: "decision"; request_id: string; behavior: Decision; detail?: DecisionDetail | null }
  | { kind: "status"; status: Status; detail?: string | null }
  | { kind: "error"; message: string };

export type SessionEvent = { seq: number; ts: number } & EventBody;

export interface Usage {
  processes: number;
  private_bytes: number;
  working_set: number;
  cpu_ms: number;
}

export type Share = "global" | "project" | "native" | "session" | "off";

export interface Server {
  id: number;
  name: string;
  share: Share;
  scope: string;
  command: string;
  launch: string | null;
  fast_path: boolean;
  pid: number | null;
  state: string;
  sessions: { conn: number; name: string; session: string | null; client_pid: number | null }[];
  restarts: number;
  requests: number;
  cache_hits: number;
  last_error: string | null;
  memory: Usage;
}

export interface Machine {
  total_bytes: number;
  available_bytes: number;
  reserve_bytes: number;
  estimate_bytes: number;
}

/** A local MCP server that Claude Code still starts on its own (not through hangar). */
export interface Unshared {
  /** `user`, or `project <folder>`. */
  scope: string;
  name: string;
}

export interface System {
  machine: Machine;
  servers: Server[];
  unshared?: Unshared[];
  /** The plan's usage limits, once a session has reported them. */
  limits?: Limits | null;
  saved_bytes: number;
  daemon_bytes: number;
  version: string;
}

export interface AppState extends System {
  sessions: Session[];
  projects: string[];
  home: string;
}

/** What a folder would run on its own when Claude starts there. */
export interface Trust {
  trusted: boolean;
  hooks: string[];
  allow_rules: string[];
  /** With their whole command lines. */
  mcp_servers: string[];
  /** Every other setting the folder makes, one line per key. */
  settings: string[];
  /** Names exactly what is listed; sent back so trust covers what was shown. */
  fingerprint: string;
}

/** What the folder picker shows for a typed path (see crates/hangar/src/folders.rs). */
export interface Folders {
  dir: string | null;
  exists: boolean;
  parent: string | null;
  entries: FolderEntry[];
  truncated: boolean;
  /** The recent folders, fresh, on the top-level listing only. */
  recent?: string[];
}

export interface FolderEntry {
  name: string;
  path: string;
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly data: Record<string, any> = {},
  ) {
    super(message);
  }
}

// The UI token comes from pairing (see store.ts) and goes in a header, never
// a cookie: cookies are shared by every port on 127.0.0.1.
let token: string | null = null;

export function setToken(t: string | null) {
  token = t;
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (token) headers["x-hangar-token"] = token;
  if (body !== undefined) headers["content-type"] = "application/json";
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(data.error ?? `${res.status} ${res.statusText}`, res.status, data);
  return data as T;
}

const q = encodeURIComponent;

export const api = {
  pair: (code: string) => call<{ token: string }>("POST", "/pair", { code }),
  state: () => call<AppState>("GET", "/state"),
  events: (id: string, after = 0) => call<SessionEvent[]>("GET", `/sessions/${id}/events?after=${after}`),
  trust: (cwd: string) => call<Trust>("GET", `/trust?cwd=${q(cwd)}`),
  folders: (path: string) => call<Folders>("GET", `/folders?path=${q(path)}`),
  searchFolders: (words: string) => call<{ entries: FolderEntry[] }>("GET", `/folders/search?q=${q(words)}`),
  create: (b: { cwd: string; prompt: string; model?: string; permission_mode?: string; trust?: boolean; trust_seen?: string; create_folder?: boolean }) =>
    call<Session>("POST", "/sessions", b),
  send: (id: string, text: string) => call("POST", `/sessions/${id}/messages`, { text }),
  decide: (id: string, requestId: string, behavior: "allow" | "allow_always" | "deny", extra: DecisionExtra = {}) =>
    call("POST", `/sessions/${id}/permissions/${q(requestId)}`, { behavior, ...extra }),
  interrupt: (id: string) => call("POST", `/sessions/${id}/interrupt`),
  stop: (id: string) => call("POST", `/sessions/${id}/stop`),
  startNow: (id: string) => call("POST", `/sessions/${id}/start`),
  update: (id: string, b: { title?: string; permission_mode?: string; model?: string }) => call<Session>("PATCH", `/sessions/${id}`, b),
  remove: (id: string) => call("DELETE", `/sessions/${id}`),
  stopServer: (id: number) => call("POST", `/servers/${id}/stop`),
  /** What `hangar wrap` (on) and `hangar unwrap` do. */
  setSharing: (on: boolean) => call<{ ok: boolean; changed: number }>("POST", "/sharing", { on }),
};

export function socketUrl() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${location.host}/api/ws`;
}

/** The subprotocols that carry the token on the WebSocket handshake. */
export function socketProtocols() {
  return token ? ["hangar", `token.${token}`] : ["hangar"];
}
