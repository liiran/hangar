// Claude Code's two requests that are not "may I run this": a question (AskUserQuestion) and a
// finished plan (ExitPlanMode). Both arrive as permission requests. These helpers read them, and
// build the answer that goes back.

export const ASK_TOOL = "AskUserQuestion";
export const PLAN_TOOL = "ExitPlanMode";

export interface Choice {
  label: string;
  description: string;
  /** A mock-up or snippet Claude attached to the option. */
  preview: string | null;
}

export interface Question {
  question: string;
  /** A word or two naming the question, such as "Sign-in". */
  header: string;
  /** Several options may be chosen. */
  multi: boolean;
  options: Choice[];
}

/**
 * The questions of an AskUserQuestion call. Null when the input is not in the shape Claude Code
 * documents: the raw request is shown then, as for any tool hangar does not know.
 */
export function questionsOf(input: unknown): Question[] | null {
  const list = (input as { questions?: unknown } | null)?.questions;
  if (!Array.isArray(list) || list.length === 0 || list.length > 8) return null;
  const out: Question[] = [];
  for (const q of list) {
    if (!q || typeof q !== "object" || typeof q.question !== "string" || !q.question.trim()) return null;
    if (!Array.isArray(q.options) || q.options.length === 0 || q.options.length > 12) return null;
    const options: Choice[] = [];
    for (const o of q.options) {
      if (!o || typeof o !== "object" || typeof o.label !== "string" || !o.label.trim()) return null;
      options.push({ label: o.label, description: typeof o.description === "string" ? o.description : "", preview: typeof o.preview === "string" && o.preview ? o.preview : null });
    }
    // Answers go back by question text and by label, so neither may repeat.
    if (new Set(options.map((o) => o.label)).size !== options.length) return null;
    out.push({ question: q.question, header: typeof q.header === "string" ? q.header : "", multi: q.multiSelect === true, options });
  }
  return new Set(out.map((q) => q.question)).size === out.length ? out : null;
}

/** What the user has chosen for one question so far. */
export interface Picked {
  /** Labels of the chosen options. */
  labels: string[];
  /** The user's own answer, typed. */
  other: string;
  /** Whether the typed answer counts. */
  otherOn: boolean;
}

export const NOTHING: Picked = { labels: [], other: "", otherOn: false };

/** Chooses or un-chooses an option. One choice replaces the last; several add up. */
export function pick(q: Question, p: Picked, label: string): Picked {
  if (!q.multi) return { ...p, labels: [label], otherOn: false };
  const on = p.labels.includes(label);
  return { ...p, labels: on ? p.labels.filter((l) => l !== label) : [...p.labels, label] };
}

/** Types an own answer. For one choice it replaces the chosen option; clearing it un-chooses it. */
export function type(q: Question, p: Picked, text: string): Picked {
  const otherOn = text.trim() !== "";
  return { labels: q.multi || !otherOn ? p.labels : [], other: text, otherOn };
}

/** Turns the typed answer on or off without losing its text. */
export function toggleOther(q: Question, p: Picked): Picked {
  const otherOn = !p.otherOn;
  return { ...p, otherOn, labels: q.multi || !otherOn ? p.labels : [] };
}

/**
 * The answer Claude gets for one question, or null when nothing is chosen. Several choices are
 * joined with ", " in the order Claude listed them, the typed one last, as Claude Code does.
 */
export function answerOf(q: Question, p: Picked): string | null {
  const typed = p.otherOn ? p.other.trim() : "";
  if (!q.multi) return typed || p.labels[0] || null;
  const parts = q.options.map((o) => o.label).filter((l) => p.labels.includes(l));
  if (typed) parts.push(typed);
  return parts.length > 0 ? parts.join(", ") : null;
}

/** Every question's answer by question text, or null until each one has an answer. */
export function answersOf(questions: Question[], picked: Picked[]): Record<string, string> | null {
  const out: Record<string, string> = {};
  for (let i = 0; i < questions.length; i++) {
    const a = answerOf(questions[i], picked[i] ?? NOTHING);
    if (a === null) return null;
    out[questions[i].question] = a;
  }
  return out;
}

/** The plan of an ExitPlanMode call and the file Claude saved it to, or null if there is none. */
export function planOf(input: unknown): { plan: string; file: string | null } | null {
  const i = input as { plan?: unknown; planFilePath?: unknown } | null;
  if (!i || typeof i.plan !== "string" || !i.plan.trim()) return null;
  return { plan: i.plan, file: typeof i.planFilePath === "string" && i.planFilePath ? i.planFilePath : null };
}

/** The modes a session can go on in once its plan is approved, in Claude Code's own words. */
export const AFTER_PLAN = [
  { mode: "acceptEdits", label: "Yes, auto-accept edits", done: "auto-accepting edits" },
  { mode: "default", label: "Yes, manually approve edits", done: "asking before each edit" },
] as const;
