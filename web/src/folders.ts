// What the folder picker lists for what was typed. Pure, so it is unit tested.

import type { Folders } from "./api";
import { matches } from "./views";

export type FolderItem =
  | { kind: "use"; path: string }
  | { kind: "create"; path: string; name: string }
  | { kind: "recent"; path: string }
  | { kind: "up"; path: string }
  | { kind: "folder"; path: string; name: string };

export interface Sections {
  /** "Use this folder" or "Create folder …". */
  top: FolderItem[];
  recent: FolderItem[];
  folders: FolderItem[];
  heading: string | null;
}

/**
 * A path the daemon can resolve on its own: absolute, or from home (`~`).
 * Anything else is a search over recent folders, never a path relative to
 * wherever the daemon happens to run.
 */
export function isPathLike(q: string): boolean {
  const t = q.trim();
  return /^[a-zA-Z]:([\\/]|$)/.test(t) || t.startsWith("\\\\") || t.startsWith("/") || t === "~" || /^~[\\/]/.test(t);
}

export function lastSegment(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}

/** The path to type to list a folder's children. */
export function into(path: string, sep: string): string {
  return /[\\/]$/.test(path) ? path : path + sep;
}

/** A folder path without a trailing separator, except for a root. */
export function tidy(path: string): string {
  const t = path.trim();
  if (/^[a-zA-Z]:[\\/]?$/.test(t)) return `${t.slice(0, 2)}\\`;
  if (t === "/") return t;
  return t.replace(/[\\/]+$/, "");
}

export function sections(query: string, listing: Folders | null, recent: string[]): Sections {
  const q = query.trim();
  const pathy = isPathLike(q);
  const top: FolderItem[] = [];
  const folders: FolderItem[] = [];
  let heading: string | null = null;
  if (listing && (pathy || q === "")) {
    if (pathy && listing.exists) top.push({ kind: "use", path: tidy(listing.dir ?? q) });
    const name = lastSegment(q);
    const taken = listing.entries.some((e) => e.name.toLowerCase() === name.toLowerCase());
    if (pathy && !listing.exists && listing.dir && name && !taken) top.push({ kind: "create", path: tidy(q), name });
    if (pathy && listing.exists && listing.parent) folders.push({ kind: "up", path: listing.parent });
    heading = listing.dir ? `In ${lastSegment(tidy(listing.dir)) || tidy(listing.dir)}` : "Places";
    for (const e of listing.entries) folders.push({ kind: "folder", path: e.path, name: e.name });
  }
  const recentItems: FolderItem[] = pathy ? [] : recent.filter((r) => matches(q, r)).slice(0, 8).map((path) => ({ kind: "recent", path }));
  return { top, recent: recentItems, folders, heading };
}
