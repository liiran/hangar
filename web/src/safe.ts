// Text from the model, tools and folders is shown to the user before they
// approve something. These helpers keep that text honest.

// Characters that are invisible or change how the text around them is laid
// out: controls, zero-width and soft characters, direction overrides, line
// separators, no-break spaces and tag characters. A command that contains
// one can read differently from what runs ("Trojan Source").
// Written as code points so that no editor or tool can alter the pattern.
const RANGES: [number, number][] = [
  [0x00, 0x08], // controls, except tab and line feed
  [0x0b, 0x1f],
  [0x7f, 0x9f],
  [0xa0, 0xa0], // no-break space
  [0xad, 0xad], // soft hyphen
  [0x61c, 0x61c], // Arabic letter mark
  [0x180e, 0x180e],
  [0x2000, 0x200f], // odd-width and zero-width spaces, direction marks
  [0x2028, 0x202f], // line separators, direction embeddings and overrides
  [0x205f, 0x206f], // word joiner, direction isolates
  [0xfeff, 0xfeff], // byte order mark
  [0xfff9, 0xfffb],
  [0xe0000, 0xe007f], // tag characters
];
const point = (n: number) => "\\u{" + n.toString(16) + "}";
const HIDDEN = new RegExp("[" + RANGES.map(([a, b]) => point(a) + "-" + point(b)).join("") + "]", "gu");

export interface Part {
  text: string;
  /** The text stands for an invisible character, as `⟨U+202E⟩`. */
  hidden: boolean;
}

/** Splits `text` so that every invisible character becomes a visible `⟨U+XXXX⟩` part. */
export function hiddenParts(text: string): Part[] {
  const parts: Part[] = [];
  // Windows line ends are ordinary; a lone carriage return is not.
  const s = text.replace(/\r\n/g, "\n");
  let last = 0;
  for (const m of s.matchAll(HIDDEN)) {
    if (m.index > last) parts.push({ text: s.slice(last, m.index), hidden: false });
    parts.push({ text: `⟨U+${m[0].codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}⟩`, hidden: true });
    last = m.index + m[0].length;
  }
  if (last < s.length) parts.push({ text: s.slice(last), hidden: false });
  return parts;
}

/** `text` with invisible characters written out. */
export function visible(text: string): string {
  return hiddenParts(text)
    .map((p) => p.text)
    .join("");
}

/** `url` if it is an absolute http(s) address, else null. Nothing else becomes a link. */
export function webUrl(url: unknown): string | null {
  if (typeof url !== "string" || !/^https?:\/\//i.test(url.trim())) return null;
  try {
    const u = new URL(url.trim());
    return (u.protocol === "http:" || u.protocol === "https:") && u.hostname ? u.href : null;
  } catch {
    return null;
  }
}

/** The host a link really goes to, to show beside link text that does not name it. */
export function linkHost(url: string, shownText: string): string | null {
  try {
    const host = new URL(url).host;
    return shownText.toLowerCase().includes(host.toLowerCase()) && !/https?:\/\//i.test(shownText.replace(url, "")) ? null : host;
  } catch {
    return null;
  }
}

/** The rules "Allow for session" would add, as Claude Code writes them: `Bash(npm test:*)`. */
export function ruleText(rule: unknown): string | null {
  if (!rule || typeof rule !== "object") return null;
  const { toolName, ruleContent } = rule as { toolName?: unknown; ruleContent?: unknown };
  if (typeof toolName !== "string" || !toolName) return null;
  return typeof ruleContent === "string" && ruleContent ? `${toolName}(${ruleContent})` : `${toolName} (every use)`;
}

/**
 * What "Allow for session" would grant, from Claude's suggestions: only
 * "allow" rule additions count (the daemon drops everything else).
 */
export function sessionRules(suggestions: unknown): string[] {
  if (!Array.isArray(suggestions)) return [];
  return suggestions
    .filter((s) => s && typeof s === "object" && s.type === "addRules" && s.behavior === "allow" && Array.isArray(s.rules))
    .flatMap((s) => (s.rules as unknown[]).map(ruleText))
    .filter((r): r is string => r !== null);
}
