import type { Status } from "./api";

export function bytes(b: number | null | undefined): string {
  if (b == null) return "—";
  const mb = b / (1024 * 1024);
  if (mb >= 1024) return `${(mb / 1024).toFixed(mb >= 10240 ? 0 : 1)} GB`;
  if (mb >= 10) return `${mb.toFixed(0)} MB`;
  return `${mb.toFixed(1)} MB`;
}

export function ago(ms: number): string {
  const s = Math.max(0, (Date.now() - ms) / 1000);
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

export function basename(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

export const STATUS: Record<Status, { label: string; tone: string; group: string }> = {
  needs_you: { label: "Needs you", tone: "amber", group: "Needs you" },
  working: { label: "Working", tone: "sky", group: "Working" },
  starting: { label: "Starting", tone: "sky", group: "Working" },
  queued: { label: "Queued", tone: "slate", group: "Queued" },
  idle: { label: "Done", tone: "emerald", group: "Ready for you" },
  hibernated: { label: "Paused", tone: "violet", group: "Paused" },
  stopped: { label: "Stopped", tone: "zinc", group: "Paused" },
  error: { label: "Error", tone: "rose", group: "Needs you" },
};

export const GROUP_ORDER = ["Needs you", "Working", "Queued", "Ready for you", "Paused"];

export const PERMISSION_MODES: { value: string; label: string; hint: string }[] = [
  { value: "default", label: "Ask first", hint: "Asks before editing files or running commands" },
  { value: "acceptEdits", label: "Auto-accept edits", hint: "Edits files freely; asks before commands" },
  { value: "plan", label: "Plan only", hint: "Reads and plans; changes nothing" },
  { value: "bypassPermissions", label: "Full access", hint: "Never asks. Only for sandboxes you trust" },
];

export const MODELS: { value: string; label: string }[] = [
  { value: "", label: "Default model" },
  { value: "opus", label: "Opus" },
  { value: "sonnet", label: "Sonnet" },
  { value: "haiku", label: "Haiku" },
];
