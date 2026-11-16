import type { Status } from "./api";

export function bytes(b: number | null | undefined): string {
  if (b == null) return "—";
  const mb = b / (1024 * 1024);
  if (mb >= 1024) return `${(mb / 1024).toFixed(mb >= 10240 ? 0 : 1)} GB`;
  if (mb >= 10) return `${mb.toFixed(0)} MB`;
  return `${mb.toFixed(1)} MB`;
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 45) return "now";
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

export function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}

export function usd(v: number): string {
  if (v <= 0) return "$0";
  return `$${v.toFixed(v < 0.1 ? 3 : 2)}`;
}

/** A token count the way Claude Code writes it: `28.9k`, `200k`, `1M`. */
export function tokens(n: number): string {
  const short = (v: number) => String(Math.round(v * 10) / 10);
  if (n >= 999_950) return `${short(n / 1_000_000)}M`;
  if (n >= 1000) return `${short(n / 1000)}k`;
  return String(Math.max(0, Math.round(n)));
}

/**
 * The color for how full something is, as a CSS color: calm below half, then warmer in steps
 * (yellow from 50%, orange from 70%, deeper from 85%) to red from 95%.
 */
export function fillColor(percent: number): string {
  if (percent >= 95) return "var(--fill-4)";
  if (percent >= 85) return "var(--fill-3)";
  if (percent >= 70) return "var(--fill-2)";
  if (percent >= 50) return "var(--fill-1)";
  return "var(--accent)";
}

/** `3:40 pm`-style time of day, in the user's own format. */
export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** When a limit starts over: the time, with the weekday when that is not today. */
export function resetTime(seconds: number, now = Date.now()): string {
  const at = new Date(seconds * 1000);
  const time = clock(at.getTime());
  return at.toDateString() === new Date(now).toDateString() ? time : `${at.toLocaleDateString([], { weekday: "short" })} ${time}`;
}

/** Claude Code's name for a usage window, in words: `five_hour` is the 5-hour limit. */
export function limitName(name: string): string {
  const known: Record<string, string> = { five_hour: "5-hour limit", seven_day: "7-day limit" };
  if (known[name]) return known[name];
  const words = name
    .replace(/^five_hour/, "5-hour")
    .replace(/^seven_day/, "7-day")
    .split("_")
    .filter(Boolean);
  return words.map((w, i) => (i > 0 ? w[0].toUpperCase() + w.slice(1) : w)).join(" ") + " limit";
}

export function basename(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** How each status reads in the app, and which list group it sorts into. */
export const STATUS: Record<Status, { label: string; group: string; hint: string }> = {
  needs_you: { label: "Needs you", group: "Needs you", hint: "Waiting for your approval" },
  error: { label: "Error", group: "Needs you", hint: "Stopped with an error" },
  working: { label: "Working", group: "Working", hint: "Claude is working" },
  starting: { label: "Starting", group: "Working", hint: "Starting Claude" },
  queued: { label: "Queued", group: "Queued", hint: "Waiting for memory" },
  idle: { label: "Ready", group: "Ready", hint: "Finished its turn; your move" },
  hibernated: { label: "Paused", group: "Paused", hint: "Paused to free memory; resumes on your next message" },
  stopped: { label: "Stopped", group: "Paused", hint: "Process stopped; resumes on your next message" },
};

export const GROUP_ORDER = ["Needs you", "Working", "Queued", "Ready", "Paused"];

export const PERMISSION_MODES: { value: string; label: string; hint: string; warn?: boolean }[] = [
  { value: "default", label: "Ask first", hint: "Asks before edits and commands" },
  { value: "acceptEdits", label: "Auto-accept edits", hint: "Edits freely; asks before commands" },
  { value: "plan", label: "Plan only", hint: "Reads and plans; changes nothing" },
  { value: "bypassPermissions", label: "Full access", hint: "Never asks. Only in a sandbox", warn: true },
];

export const MODELS: { value: string; label: string; hint: string }[] = [
  { value: "", label: "Default model", hint: "Claude Code's own default" },
  { value: "opus", label: "Opus", hint: "Most capable; slowest" },
  { value: "sonnet", label: "Sonnet", hint: "Fast and strong for code" },
  { value: "haiku", label: "Haiku", hint: "Fastest; for small tasks" },
];

/** The picker choice for a model name: a full id such as claude-sonnet-5-5 maps to its family. */
export function modelChoice(model: string | null | undefined): string {
  const m = (model ?? "").toLowerCase();
  return MODELS.find((o) => o.value && m.includes(o.value))?.value ?? "";
}
