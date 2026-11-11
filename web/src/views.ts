// Which sessions a view shows, in what order. Pure, so it is unit tested.

import type { Session } from "./api";
import { basename, GROUP_ORDER, STATUS } from "./format";

export type View = { kind: "inbox" } | { kind: "all"; project: string | null } | { kind: "pool" };

/** A folder's identity: case- and separator-insensitive (Windows paths). */
function pathKey(p: string): string {
  return p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

export function samePath(a: string, b: string): boolean {
  return pathKey(a) === pathKey(b);
}

/** Sessions that need you: a permission request or an error. */
export function needsYou(s: Session): boolean {
  return s.status === "needs_you" || s.status === "error";
}

const groupRank = (s: Session) => GROUP_ORDER.indexOf(STATUS[s.status].group);

/** The sessions a view lists, grouped by status, most recent first. */
export function visible(sessions: Iterable<Session>, view: View): Session[] {
  const all = [...sessions];
  const shown =
    view.kind === "inbox" ? all.filter(needsYou) : view.kind === "all" && view.project ? all.filter((s) => samePath(s.cwd, view.project!)) : all;
  return shown.sort((a, b) => groupRank(a) - groupRank(b) || b.updated_ms - a.updated_ms);
}

/** Consecutive runs of `list` (already sorted by `visible`) by status group. */
export function groups(list: Session[]): [string, Session[]][] {
  const out: [string, Session[]][] = [];
  for (const s of list) {
    const g = STATUS[s.status].group;
    if (out.length && out[out.length - 1][0] === g) out[out.length - 1][1].push(s);
    else out.push([g, [s]]);
  }
  return out;
}

export interface Project {
  path: string;
  name: string;
  count: number;
  updated: number;
}

/** Folders that have sessions, most recently active first. */
export function projects(sessions: Iterable<Session>): Project[] {
  const by = new Map<string, Project>();
  for (const s of sessions) {
    const key = pathKey(s.cwd);
    const p = by.get(key) ?? { path: s.cwd, name: basename(s.cwd), count: 0, updated: 0 };
    p.count++;
    p.updated = Math.max(p.updated, s.updated_ms);
    by.set(key, p);
  }
  return [...by.values()].sort((a, b) => b.updated - a.updated);
}

/** Every word of the query appears in one of the fields (case-insensitive). */
export function matches(query: string, ...fields: (string | null | undefined)[]): boolean {
  const hay = fields.filter(Boolean).join(" ").toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

/** The session after (or before) `current` in `list`, wrapping at neither end. */
export function step(list: Session[], current: string | null, delta: 1 | -1): string | null {
  if (list.length === 0) return null;
  const i = list.findIndex((s) => s.id === current);
  if (i < 0) return list[delta === 1 ? 0 : list.length - 1].id;
  return list[Math.min(list.length - 1, Math.max(0, i + delta))].id;
}
