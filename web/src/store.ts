import { create } from "zustand";
import { api, ApiError, setToken, socketProtocols, socketUrl, type Session, type SessionEvent, type System } from "./api";
import { visible } from "./safe";
import { isNewer, mergeEvents, notice, pairCode } from "./sync";

type Conn = "connecting" | "live" | "offline" | "unauthorized";

interface Toast {
  id: number;
  text: string;
}

interface Store {
  conn: Conn;
  /** Why the window is not signed in, when it is not. */
  authError: string | null;
  sessions: Record<string, Session>;
  events: Record<string, SessionEvent[]>;
  system: System | null;
  projects: string[];
  home: string;
  selected: string | null;
  toasts: Toast[];
  select: (id: string | null) => void;
  toast: (text: string) => void;
  dismiss: (id: number) => void;
}

export const useStore = create<Store>((set) => ({
  conn: "connecting",
  authError: null,
  sessions: {},
  events: {},
  system: null,
  projects: [],
  home: "",
  selected: null,
  toasts: [],
  select: (id) => {
    set({ selected: id });
    if (id) {
      saved.set(SELECTED_KEY, id);
      loadEvents(id);
    }
  },
  toast: (text) => {
    const id = Date.now() + Math.random();
    set((s) => ({ toasts: [...s.toasts, { id, text }] }));
    setTimeout(() => useStore.getState().dismiss(id), 6000);
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

export function report(e: unknown) {
  useStore.getState().toast(e instanceof Error ? e.message : String(e));
}

// ------------------------------------------------------------ local storage

const TOKEN_KEY = "hangar.token";
const SELECTED_KEY = "hangar.selected";

/** localStorage that never throws (private windows, blocked storage). */
export const saved = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key: string, value: string | null) {
    try {
      if (value == null) localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    } catch {}
  },
};

// ------------------------------------------------------------ sign-in

/**
 * Signs the window in. `hangar up` opens `/#pair=<code>`: the one-time code
 * is traded for the UI token, and removed from the address bar and history.
 */
export async function start() {
  const code = pairCode(location.hash);
  if (code) {
    history.replaceState(null, "", location.pathname + location.search);
    try {
      const { token } = await api.pair(code);
      saved.set(TOKEN_KEY, token);
    } catch (e) {
      // An old link: fine if this window is already signed in.
      if (!saved.get(TOKEN_KEY)) return signedOut(e instanceof Error ? e.message : String(e));
    }
  }
  const token = saved.get(TOKEN_KEY);
  if (!token) return signedOut(null);
  setToken(token);
  connect();
}

// A new link opened in a tab that already shows hangar only changes the hash.
if (typeof window !== "undefined") {
  window.addEventListener("hashchange", () => {
    if (pairCode(location.hash)) start();
  });
}

function signedOut(reason: string | null) {
  saved.set(TOKEN_KEY, null);
  setToken(null);
  socket?.close();
  socket = null;
  useStore.setState({ conn: "unauthorized", authError: reason });
}

// ------------------------------------------------------------ live sync

let socket: WebSocket | null = null;
let retry = 0;
/** Bumped on every full resync; late answers from older ones are dropped. */
let generation = 0;
/** Sessions whose events are being fetched, with events that arrived meanwhile. */
const loading = new Map<string, SessionEvent[]>();

/**
 * Opens the socket first and buffers what it sends until the snapshot is
 * in, so nothing that happens in between is lost.
 */
function connect() {
  socket?.close();
  const ws = new WebSocket(socketUrl(), socketProtocols());
  socket = ws;
  let buffer: unknown[] | null = [];
  const resync = async () => {
    buffer ??= [];
    try {
      await refresh();
    } finally {
      const queued = buffer ?? [];
      buffer = null;
      queued.forEach(handle);
    }
  };
  ws.onopen = () => {
    retry = 0;
    resync().catch(() => ws.close());
  };
  ws.onmessage = (m) => {
    let msg: any;
    try {
      msg = JSON.parse(m.data);
    } catch {
      return;
    }
    if (msg?.type === "resync") resync().catch(() => ws.close());
    else if (buffer) buffer.push(msg);
    else handle(msg);
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    useStore.setState((s) => (s.conn === "unauthorized" ? {} : { conn: "offline" }));
    // A refused handshake looks like any other close: ask the API why.
    api
      .state()
      .then(() => setTimeout(connect, backoff()))
      .catch((e) => {
        if (e instanceof ApiError && e.status === 401) signedOut(e.message);
        else setTimeout(connect, backoff());
      });
  };
}

function backoff() {
  retry = Math.min(retry + 1, 6);
  return 250 * 2 ** retry;
}

/** Replaces everything with a fresh snapshot. */
async function refresh() {
  let st;
  try {
    st = await api.state();
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) signedOut(e.message);
    throw e;
  }
  generation++;
  loading.clear();
  const sessions: Record<string, Session> = {};
  for (const s of st.sessions) sessions[s.id] = s;
  const { sessions: _, projects, home, ...system } = st;
  useStore.setState((s) => {
    let selected = s.selected && sessions[s.selected] ? s.selected : null;
    // Open the last session, except on a phone-sized screen, where a
    // session covers the list: start on the list there.
    const roomy = typeof window.matchMedia !== "function" || window.matchMedia("(min-width: 768px)").matches;
    if (!selected && roomy) {
      const last = saved.get(SELECTED_KEY);
      selected = last && sessions[last] ? last : (st.sessions[0]?.id ?? null);
    }
    // Cached transcripts may have missed events while disconnected. Keep the
    // open one on screen (logs are append-only) and catch it up below.
    const events: Record<string, SessionEvent[]> = {};
    if (selected && s.events[selected]) events[selected] = s.events[selected];
    return { sessions, system, projects, home, selected, events, conn: "live", authError: null };
  });
  const sel = useStore.getState().selected;
  if (sel) loadEvents(sel, true);
}

/** Fetches a session's events (only the new ones if some are cached). */
export async function loadEvents(id: string, force = false) {
  const have = useStore.getState().events[id];
  if ((have && !force) || loading.has(id)) return;
  const gen = generation;
  loading.set(id, []);
  try {
    const fetched = await api.events(id, have?.at(-1)?.seq ?? 0);
    if (gen !== generation) return;
    const meanwhile = loading.get(id) ?? [];
    useStore.setState((s) => ({ events: { ...s.events, [id]: mergeEvents(s.events[id] ?? [], [...fetched, ...meanwhile]) } }));
  } catch (e) {
    if (gen === generation) report(e);
  } finally {
    if (gen === generation) loading.delete(id);
  }
}

/** Applies a session view from any source, unless a newer one is in. */
export function upsertSession(s: Session) {
  const prev = useStore.getState().sessions[s.id];
  if (!isNewer(prev, s)) return;
  notify(prev, s);
  useStore.setState((st) => ({ sessions: { ...st.sessions, [s.id]: s } }));
}

function handle(msg: any) {
  switch (msg?.type) {
    case "session":
      upsertSession(msg.session);
      break;
    case "event": {
      const id: string = msg.session_id;
      const pending = loading.get(id);
      if (pending) pending.push(msg.event);
      else useStore.setState((st) => (st.events[id] ? { events: { ...st.events, [id]: mergeEvents(st.events[id], [msg.event]) } } : {}));
      break;
    }
    case "removed":
      useStore.setState((st) => {
        const sessions = { ...st.sessions };
        const events = { ...st.events };
        delete sessions[msg.session_id];
        delete events[msg.session_id];
        return { sessions, events, selected: st.selected === msg.session_id ? null : st.selected };
      });
      break;
    case "system": {
      const { type: _, ...system } = msg;
      useStore.setState({ system });
      break;
    }
  }
}

// ------------------------------------------------------------ notifications

function notify(prev: Session | undefined, next: Session) {
  const kind = notice(prev, next);
  if (!kind) return;
  if (document.visibilityState === "visible" && useStore.getState().selected === next.id) return;
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const name = visible(next.title);
  const title = kind === "needs_you" ? `${name} needs you` : kind === "failed" ? `${name} failed` : `${name} is done`;
  // Only what hangar itself knows: the model's words and error text stay
  // in the app, not in the notification center or on a lock screen.
  const body = kind === "needs_you" ? `Wants to use ${visible(next.pending[0]?.tool_name ?? "a tool").slice(0, 60)}` : kind === "failed" ? "Stopped with an error" : "Finished its turn";
  const n = new Notification(title, { body, tag: next.id });
  n.onclick = () => {
    window.focus();
    useStore.getState().select(next.id);
    n.close();
  };
}

export function askNotificationPermission() {
  if (typeof Notification !== "undefined" && Notification.permission === "default") {
    Notification.requestPermission().catch(() => {});
  }
}
