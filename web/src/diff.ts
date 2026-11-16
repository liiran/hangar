// The lines an edit changed, for the feed.
//
// Claude Code reports every Edit and Write as a patch: for each changed stretch, the line it
// starts at and its lines, unchanged ones around it included. That is shown as it came. An edit
// that never ran (it failed, or was refused) has no report, so the change that was asked for is
// compared line by line here instead, without line numbers.

export interface DiffLine {
  /** `+` added, `-` removed, ` ` unchanged, `\` a note from the patch ("No newline at end of file"). */
  sign: "+" | "-" | " " | "\\";
  text: string;
  /** The line's number in the file: after the change, or before it for a removed line. */
  n: number | null;
}

export interface FileDiff {
  /** One list of lines per changed stretch of the file. */
  hunks: DiffLine[][];
  added: number;
  removed: number;
  /** The patch was longer than what was kept. */
  truncated: boolean;
}

function finish(hunks: DiffLine[][], truncated = false): FileDiff {
  let added = 0;
  let removed = 0;
  for (const h of hunks) {
    for (const l of h) {
      if (l.sign === "+") added++;
      else if (l.sign === "-") removed++;
    }
  }
  return { hunks, added, removed, truncated };
}

/** Claude Code's report of an edit (`tool_use_result`, as the daemon keeps it), or null if it has no changed lines. */
export function fromPatch(result: unknown): FileDiff | null {
  const patch = (result as { structuredPatch?: unknown } | null)?.structuredPatch;
  if (!Array.isArray(patch)) return null;
  const hunks: DiffLine[][] = [];
  for (const h of patch) {
    if (!h || typeof h !== "object" || !Array.isArray(h.lines)) continue;
    let before = Number.isInteger(h.oldStart) ? (h.oldStart as number) : null;
    let after = Number.isInteger(h.newStart) ? (h.newStart as number) : null;
    const lines: DiffLine[] = [];
    for (const raw of h.lines) {
      if (typeof raw !== "string") continue;
      const sign = raw[0];
      const text = raw.slice(1);
      if (sign === "+") lines.push({ sign, text, n: after === null ? null : after++ });
      else if (sign === "-") lines.push({ sign, text, n: before === null ? null : before++ });
      else if (sign === "\\") lines.push({ sign, text: text.trim(), n: null });
      else {
        lines.push({ sign: " ", text, n: after });
        if (after !== null) after++;
        if (before !== null) before++;
      }
    }
    if (lines.length > 0) hunks.push(lines);
  }
  return hunks.length > 0 ? finish(hunks, (result as { truncated?: unknown }).truncated === true) : null;
}

/** A new file: every line added, numbered from 1. */
export function fromContent(content: string): FileDiff {
  const lines = content.split("\n");
  // A last line break ends the last line; it does not start an empty one.
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return finish([lines.map((text, i) => ({ sign: "+" as const, text, n: i + 1 }))]);
}

/** Above this many line pairs, the two texts are shown one after the other instead of compared. */
const MAX_COMPARED = 250_000;

/** `before` turned into `after`, line by line, without line numbers (where in the file is not known). */
export function fromEdit(before: string, after: string): FileDiff {
  const a = before.split("\n");
  const b = after.split("\n");
  // Lines the two share at both ends need no comparing.
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const x = a.slice(head, a.length - tail);
  const y = b.slice(head, b.length - tail);
  const same = (text: string): DiffLine => ({ sign: " ", text, n: null });
  const out: DiffLine[] = a.slice(0, head).map(same);
  if (x.length * y.length > MAX_COMPARED) {
    out.push(...x.map((text): DiffLine => ({ sign: "-", text, n: null })), ...y.map((text): DiffLine => ({ sign: "+", text, n: null })));
  } else {
    // The longest run of lines both keep, in order (a table of lengths, read back from the start).
    const w = y.length + 1;
    const len = new Uint32Array((x.length + 1) * w);
    for (let i = x.length - 1; i >= 0; i--) {
      for (let j = y.length - 1; j >= 0; j--) {
        len[i * w + j] = x[i] === y[j] ? len[(i + 1) * w + j + 1] + 1 : Math.max(len[(i + 1) * w + j], len[i * w + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < x.length || j < y.length) {
      if (i < x.length && j < y.length && x[i] === y[j]) {
        out.push(same(x[i]));
        i++;
        j++;
      } else if (i < x.length && (j >= y.length || len[(i + 1) * w + j] >= len[i * w + j + 1])) {
        out.push({ sign: "-", text: x[i++], n: null });
      } else {
        out.push({ sign: "+", text: y[j++], n: null });
      }
    }
  }
  out.push(...a.slice(a.length - tail).map(same));
  return finish([out]);
}

/** What a finished file tool call changed, or (if it failed) was meant to change. Null for other tools. */
export function diffOf(name: string, input: any, result: { isError: boolean; patch?: unknown } | undefined): FileDiff | null {
  if (!result || !input || typeof input !== "object") return null;
  if (!result.isError) {
    const reported = fromPatch(result.patch);
    if (reported) return reported;
    // A file written for the first time has nothing to compare with: its content is the change.
    const wroteNew = name === "Write" && (result.patch as { type?: unknown } | null | undefined)?.type !== "update";
    return wroteNew && typeof input.content === "string" ? fromContent(input.content) : null;
  }
  if (name === "Edit" && typeof input.old_string === "string" && typeof input.new_string === "string") return fromEdit(input.old_string, input.new_string);
  if (name === "MultiEdit" && Array.isArray(input.edits)) {
    const parts = input.edits.filter((e: any) => e && typeof e.old_string === "string" && typeof e.new_string === "string").map((e: any) => fromEdit(e.old_string, e.new_string).hunks[0]);
    return parts.length > 0 ? finish(parts) : null;
  }
  if (name === "Write" && typeof input.content === "string") return fromContent(input.content);
  return null;
}
