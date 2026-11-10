import { create } from "zustand";
import { api, ApiError, type Session, type SessionEvent, type System } from "./api";

type Conn = "connecting" | "live" | "offline" | "unauthorized";

interface Toast {
  id: number;
  text: string;
}

interface Store {
  conn: Conn;
  sessions: Record<string, Session>;
  events: Record<string, SessionEvent[]>;
  system: System | null;
  projects: string[];
  home: string;
  selected: string | null;
  composing: boolean;
  toasts: Toast[];
  select: (id: string | null) => void;
  setComposing: (v: boolean) => void;
  toast: (text: string) => void;
  dismiss: (id: number) => void;
}

export const useStore = create<Store>((set) => ({
  conn: "connecting",
  sessions: {},
  events: {},
  system: null,
  projects: [],
  home: "",
  selected: null,
  composing: false,
  toasts: [],
  select: (id) => {
    set({ selected: id, composing: false });
    if (id) {
      try {
        localStorage.setItem("hangar.selected", id);
      } catch {}
      loadEvents(id);
    }
  },
  setComposing: (v) => set({ composing: v }),
  toast: (text) => {
    const id = Date.now() + Math.random();
    set((s) => ({ toasts: [...s.toasts, { id, text }] }));
    setTimeout(() => useStore.getState().dismiss(id), 6000);
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

const loading = new Set<string>();

export async function loadEvents(id: string) {
  if (useStore.getState().events[id] || loading.has(id)) return;
  loading.add(id);
  try {
    const evs = await api.events(id);
    useStore.setState((s) => ({ events: { ...s.events, [id]: mergeEvents(s.events[id] ?? [], evs) } }));
  } catch (e) {
    report(e);
  } finally {
    loading.delete(id);
  }
}

function mergeEvents(have: SessionEvent[], incoming: SessionEvent[]): SessionEvent[] {
  if (incoming.length === 0) return have;
  const seen = new Set(have.map((e) => e.seq));
  const merged = have.concat(incoming.filter((e) => !seen.has(e.seq)));
  merged.sort((a, b) => a.seq - b.seq);
  return merged;
}

export function report(e: unknown) {
  const text = e instanceof Error ? e.message : String(e);
  useStore.getState().toast(text);
}

async function refresh() {
  try {
    const st = await api.state();
    const sessions: Record<string, Session> = {};
    for (const s of st.sessions) sessions[s.id] = s;
    const { sessions: _, projects, home, ...system } = st;
    useStore.setState((s) => {
      // Keep the selection if it still exists; otherwise restore the last one.
      let selected = s.selected && sessions[s.selected] ? s.selected : null;
      if (!selected) {
        let saved: string | null = null;
        try {
          saved = localStorage.getItem("hangar.selected");
        } catch {}
        selected = saved && sessions[saved] ? saved : st.sessions[0]?.id ?? null;
      }
      return { sessions, system, projects, home, selected, conn: "live" };
    });
    const sel = useStore.getState().selected;
    if (sel) {
      // Refetch the open transcript in full after a reconnect.
      useStore.setState((s) => {
        const events = { ...s.events };
        delete events[sel];
        return { events };
      });
      loadEvents(sel);
    }
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) useStore.setState({ conn: "unauthorized" });
    else useStore.setState({ conn: "offline" });
    throw e;
  }
}

let socket: WebSocket | null = null;
let retry = 0;

export function connect() {
  refresh()
    .then(openSocket)
    .catch(() => {
      if (useStore.getState().conn !== "unauthorized") setTimeout(connect, backoff());
    });
}

function backoff() {
  retry = Math.min(retry + 1, 6);
  return 250 * 2 ** retry;
}

function openSocket() {
  socket?.close();
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  socket = ws;
  ws.onopen = () => {
    retry = 0;
    useStore.setState({ conn: "live" });
  };
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    switch (msg.type) {
      case "session": {
        const s: Session = msg.session;
        const prev = useStore.getState().sessions[s.id];
        notifyTransition(prev, s);
        useStore.setState((st) => ({ sessions: { ...st.sessions, [s.id]: s } }));
        break;
      }
      case "event": {
        const id: string = msg.session_id;
        useStore.setState((st) => (st.events[id] ? { events: { ...st.events, [id]: mergeEvents(st.events[id], [msg.event]) } } : {}));
        break;
      }
      case "removed": {
        useStore.setState((st) => {
          const sessions = { ...st.sessions };
          delete sessions[msg.session_id];
          return { sessions, selected: st.selected === msg.session_id ? null : st.selected };
        });
        break;
      }
      case "system": {
        const { type: _, ...system } = msg;
        useStore.setState({ system });
        break;
      }
      case "resync":
        refresh().catch(() => {});
        break;
    }
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    useStore.setState({ conn: "offline" });
    setTimeout(connect, backoff());
  };
}

// ------------------------------------------------------------ notifications

function notifyTransition(prev: Session | undefined, next: Session) {
  if (!prev || prev.status === next.status) return;
  const asks = next.status === "needs_you";
  const done = next.status === "idle" && (prev.status === "working" || prev.status === "starting");
  if (!asks && !done) return;
  if (document.visibilityState === "visible" && useStore.getState().selected === next.id) return;
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const n = new Notification(asks ? `${next.title} needs you` : `${next.title} is done`, {
    body: asks ? `Wants to use ${next.pending[0]?.tool_name ?? "a tool"}` : next.preview || "Finished its turn",
    tag: next.id,
  });
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
