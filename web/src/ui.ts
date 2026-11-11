// Where the user is in the app and which panels are open. Kept apart from
// the data store, which mirrors the daemon.

import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { saved, useStore } from "./store";
import { type View, visible } from "./views";

type Overlay = null | "new" | "palette";

interface UI {
  view: View;
  /** Wide screens: the sidebar is collapsed (remembered). */
  navHidden: boolean;
  /** Narrow screens: the sidebar drawer is open. */
  navDrawer: boolean;
  /** The session properties panel (remembered). */
  props: boolean;
  overlay: Overlay;
  setView: (v: View) => void;
  toggleNav: () => void;
  closeDrawer: () => void;
  toggleProps: () => void;
  open: (o: Overlay) => void;
}

const WIDE_NAV = "(min-width: 1024px)";

const wide = (query: string) => typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(query).matches;

export const useUI = create<UI>((set) => ({
  view: { kind: "all", project: null },
  navHidden: saved.get("hangar.navHidden") === "1",
  navDrawer: false,
  props: saved.get("hangar.props") !== "0",
  overlay: null,
  setView: (view) => set({ view, navDrawer: false }),
  toggleNav: () =>
    set((s) => {
      if (!wide(WIDE_NAV)) return { navDrawer: !s.navDrawer };
      saved.set("hangar.navHidden", s.navHidden ? "0" : "1");
      return { navHidden: !s.navHidden };
    }),
  closeDrawer: () => set({ navDrawer: false }),
  toggleProps: () =>
    set((s) => {
      saved.set("hangar.props", s.props ? "0" : "1");
      return { props: !s.props };
    }),
  open: (overlay) => set({ overlay, navDrawer: false }),
}));

/** Set when a session is opened by pointer or palette: its composer takes focus. */
export const intent = { focusComposer: false };

export function focusComposer() {
  document.querySelector<HTMLTextAreaElement>("textarea[data-composer]")?.focus();
}

/**
 * Opens a session the user picked (click or palette) and puts the cursor in
 * its reply box. Keyboard navigation (J/K) uses `select` and leaves focus
 * alone, so the next keystroke still navigates.
 */
export function openSession(id: string) {
  if (useStore.getState().selected === id) return focusComposer();
  intent.focusComposer = true;
  useStore.getState().select(id);
}

/** The current time, refreshed every `every` ms, so relative times stay true. */
export function useNow(every = 30_000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(t);
  }, [every]);
  return now;
}

/** The sessions the current view lists, in display order. */
export function useVisible() {
  const sessions = useStore((s) => s.sessions);
  const view = useUI((s) => s.view);
  return useMemo(() => visible(Object.values(sessions), view), [sessions, view]);
}
