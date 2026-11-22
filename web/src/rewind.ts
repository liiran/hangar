// What the Rewind dialog shows: the messages that can be gone back to, and Claude Code's own
// words for what a restore will do (read from its terminal's code, 2.1.329).

import type { RewindPreview } from "./api";
import { relative, type Row } from "./transcript";

/** One of the user's messages, as a point to go back to. */
export interface RewindPoint {
  /** The message's number in the session's history. */
  seq: number;
  text: string;
  ts: number;
  /** hangar sent it with an id Claude Code can rewind to. Messages from before hangar did cannot be gone back to. */
  canRewind: boolean;
  /** The files Claude edited while answering it, as the feed shows them (not counting helper agents, which rewind does not cover). */
  edited: string[];
  /** Sent while a turn ran: Claude Code kept no file checkpoint for it. */
  midTurn: boolean;
}

const EDITS = new Set(["Edit", "Write", "MultiEdit", "NotebookEdit"]);

/** The user's messages still in the conversation, oldest first, each with the files Claude edited in its turn. */
export function rewindPoints(rows: Row[], cwd: string): RewindPoint[] {
  const points: RewindPoint[] = [];
  for (const r of rows) {
    if (r.kind === "user") points.push({ seq: r.seq, text: r.text, ts: r.ts, canRewind: r.canRewind, edited: [], midTurn: !!r.midTurn });
    else if (r.kind === "tool" && EDITS.has(r.name) && r.result && !r.result.isError && points.length > 0) {
      const path = r.input?.file_path ?? r.input?.notebook_path;
      const name = typeof path === "string" ? relative(path, cwd) : "";
      const edited = points[points.length - 1].edited;
      if (name && !edited.includes(name)) edited.push(name);
    }
  }
  return points;
}

const base = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

/** "a.txt", "a.txt and b.txt", "a.txt and 2 other files": how Claude Code names the files a restore changes. */
export function fileNames(files: string[]): string {
  const names = files.map(base);
  if (names.length <= 1) return names[0] ?? "";
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names[0]} and ${names.length - 1} other files`;
}

/** Under a message in the list: what Claude edited in that turn. hangar's line, from the feed. */
export function editedLine(point: RewindPoint): string {
  if (!point.canRewind) return "Sent before hangar could rewind";
  // Claude Code's words for a message it took into a running turn.
  if (point.midTurn) return "⚠ No code restore";
  if (point.edited.length === 0) return "No edits";
  return point.edited.length === 1 ? `Edited ${base(point.edited[0])}` : `Edited ${point.edited.length} files`;
}

export type RewindChoice = "both" | "conversation" | "code" | "nevermind";

/** Whether the preview found files a restore would change: only then does Claude Code offer the two "code" choices. */
export const canRestoreCode = (preview: RewindPreview | null) => !!preview && preview.can_restore && preview.files.length > 0;

/** The choices, in Claude Code's order. Its two "Summarize" choices are left out: no request of Claude Code's does them. */
export function rewindChoices(preview: RewindPreview | null): { value: RewindChoice; label: string }[] {
  const code = canRestoreCode(preview);
  return [
    ...(code ? [{ value: "both" as const, label: "Restore code and conversation" }] : []),
    { value: "conversation" as const, label: "Restore conversation" },
    ...(code ? [{ value: "code" as const, label: "Restore code" }] : []),
    { value: "nevermind" as const, label: "Never mind" },
  ];
}

/** Claude Code's two lines about the highlighted choice: what happens to the conversation, and to the code. */
export function choiceLines(choice: RewindChoice, preview: RewindPreview | null): [string, string] {
  const conversation = choice === "both" || choice === "conversation" ? "The conversation goes back to before this message." : "The conversation will be unchanged.";
  if (!canRestoreCode(preview)) return [conversation, "The code has not changed (nothing will be restored)."];
  if (choice !== "both" && choice !== "code") return [conversation, "The code will be unchanged."];
  return [conversation, `The code will be restored +${preview!.insertions} -${preview!.deletions} in ${fileNames(preview!.files)}.`];
}

/** Claude Code's warning under the choices when code can be restored. */
export const RESTORE_WARNING = "Restored files go back whole, losing later changes by anyone. Bash changes to other files are not undone.";

/** What to say after a restore that did not fully work, in Claude Code's words. Empty when all went well. */
export function restoreProblems(done: {
  code?: { ok: boolean; skipped?: number; error?: string };
  conversation?: { ok: boolean; error?: string };
}): string[] {
  const { code, conversation } = done;
  const out: string[] = [];
  if (code && !code.ok && conversation && !conversation.ok) out.push(`Failed to restore the conversation and code: ${code.error ?? ""} ${conversation.error ?? ""}`.trim());
  else {
    if (code && !code.ok) out.push(`Failed to restore the code: ${code.error ?? ""}`.trim());
    if (conversation && !conversation.ok) out.push(`Failed to restore the conversation: ${conversation.error ?? ""}`.trim());
  }
  const skipped = code?.ok ? (code.skipped ?? 0) : 0;
  if (skipped > 0) {
    out.push(
      `Restored the code, but skipped ${skipped} file(s): the tracked path is (or became) a link or other non-regular file, its directory changed since the checkpoint, it is behind a link in a folder that sandboxed commands can write, or its backup could not be safely read. Skipped files were left untouched.`,
    );
  }
  return out;
}
