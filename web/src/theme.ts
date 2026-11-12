// The app's look and how it is installed: the theme follows the system
// unless the user pins light or dark, and the browser may offer to install
// hangar as an app of its own.
import { create } from "zustand";
import { saved } from "./store";

export type ThemeChoice = "system" | "light" | "dark";
export const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "Match the system" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const KEY = "hangar.theme";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");
/** The window color (--bg) of each theme, for an installed app's title bar. */
const WINDOW = { light: "#f3f3f4", dark: "#08090a" };

function apply(choice: ThemeChoice) {
  const theme = choice === "system" ? (systemDark.matches ? "dark" : "light") : choice;
  document.documentElement.dataset.theme = theme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", WINDOW[theme]);
}

export const useTheme = create<{ choice: ThemeChoice; choose: (choice: ThemeChoice) => void }>((set) => ({
  choice: THEMES.find((t) => t.value === saved.get(KEY))?.value ?? "system",
  choose: (choice) => {
    saved.set(KEY, choice === "system" ? null : choice);
    apply(choice);
    set({ choice });
  },
}));

/** What the browser hands over when the page can be installed as an app. */
interface InstallPrompt extends Event {
  prompt: () => Promise<unknown>;
}

export const useInstall = create<{ offer: InstallPrompt | null; install: () => void }>((set, get) => ({
  offer: null,
  install: () => {
    get()
      .offer?.prompt()
      .catch(() => {});
    set({ offer: null });
  },
}));

/** The part of the browser that reports a hidden title bar (installed apps only). */
interface Overlay extends EventTarget {
  visible: boolean;
}

/** Applies the theme before the first render and keeps following the system. */
export function startTheme() {
  apply(useTheme.getState().choice);
  systemDark.addEventListener("change", () => apply(useTheme.getState().choice));
  // With the title bar hidden the page draws the top of the window itself.
  const overlay = (navigator as { windowControlsOverlay?: Overlay }).windowControlsOverlay;
  const top = () => document.documentElement.classList.toggle("overlay", !!overlay?.visible);
  top();
  overlay?.addEventListener("geometrychange", top);
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    useInstall.setState({ offer: e as InstallPrompt });
  });
  window.addEventListener("appinstalled", () => useInstall.setState({ offer: null }));
}
