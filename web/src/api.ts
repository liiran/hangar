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
}

export type Decision = "allow" | "allow_always" | "deny" | "cancelled";

export type EventBody =
  | { kind: "user"; text: string }
  | { kind: "claude"; msg: any }
  | { kind: "permission"; request_id: string; tool_name: string; input: any; suggestions?: unknown; description?: string | null }
  | { kind: "decision"; request_id: string; behavior: Decision }
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

export interface System {
  machine: Machine;
  servers: Server[];
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
  mcp_servers: string[];
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
  create: (b: { cwd: string; prompt: string; model?: string; permission_mode?: string; trust?: boolean; create_folder?: boolean }) =>
    call<Session>("POST", "/sessions", b),
  send: (id: string, text: string) => call("POST", `/sessions/${id}/messages`, { text }),
  decide: (id: string, requestId: string, behavior: "allow" | "allow_always" | "deny") => call("POST", `/sessions/${id}/permissions/${q(requestId)}`, { behavior }),
  interrupt: (id: string) => call("POST", `/sessions/${id}/interrupt`),
  stop: (id: string) => call("POST", `/sessions/${id}/stop`),
  startNow: (id: string) => call("POST", `/sessions/${id}/start`),
  update: (id: string, b: { title?: string; permission_mode?: string }) => call<Session>("PATCH", `/sessions/${id}`, b),
  remove: (id: string) => call("DELETE", `/sessions/${id}`),
  stopServer: (id: number) => call("POST", `/servers/${id}/stop`),
};

export function socketUrl() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${location.host}/api/ws`;
}

/** The subprotocols that carry the token on the WebSocket handshake. */
export function socketProtocols() {
  return token ? ["hangar", `token.${token}`] : ["hangar"];
}
