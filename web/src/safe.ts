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
  [0x34f, 0x34f], // combining grapheme joiner
  [0x61c, 0x61c], // Arabic letter mark
  [0x115f, 0x1160], // Hangul fillers: blank, and valid in a variable name
  [0x1680, 0x1680], // a space that is not the ordinary one
  [0x17b4, 0x17b5], // invisible Khmer vowels
  [0x180e, 0x180e],
  [0x2000, 0x200f], // odd-width and zero-width spaces, direction marks
  [0x2028, 0x202f], // line separators, direction embeddings and overrides
  [0x205f, 0x206f], // word joiner, direction isolates
  [0x2800, 0x2800], // blank braille pattern
  [0x3000, 0x3000], // wide space
  [0x3164, 0x3164], // Hangul filler
  [0xfe00, 0xfe0f], // variation selectors
  [0xfeff, 0xfeff], // byte order mark
  [0xffa0, 0xffa0], // halfwidth Hangul filler
  [0xfff9, 0xfffb],
  [0xe0000, 0xe007f], // tag characters
  [0xe0100, 0xe01ef], // more variation selectors
];
// Letters of right-to-left scripts (Hebrew, Arabic and others).
const RTL_RANGES: [number, number][] = [
  [0x590, 0x8ff],
  [0xfb1d, 0xfdff],
  [0xfe70, 0xfefc],
  [0x10800, 0x10fff],
  [0x1e800, 0x1efff],
];
const point = (n: number) => "\\u{" + n.toString(16) + "}";
const HIDDEN = new RegExp("[" + RANGES.map(([a, b]) => point(a) + "-" + point(b)).join("") + "]", "gu");
const RTL = new RegExp("[" + RTL_RANGES.map(([a, b]) => point(a) + "-" + point(b)).join("") + "]", "u");

/**
 * Whether `text` has right-to-left letters. They are ordinary, visible letters, but the screen
 * shows them, and digits and punctuation next to them, in another order than they are written.
 */
export function rightToLeft(text: string): boolean {
  return RTL.test(text);
}

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

/** The host a link really goes to, to show beside link text that does not name exactly that site. */
export function linkHost(url: string, shownText: string): string | null {
  let target: URL;
  try {
    target = new URL(url);
  } catch {
    return null;
  }
  const host = target.host.toLowerCase();
  // A name before "@" is not the site: https://github.com@evil.example goes to evil.example.
  if (target.username || target.password) return target.host;
  const text = shownText.trim().toLowerCase();
  // Text that is itself an address must lead to the same site.
  if (/^[a-z][a-z0-9+.-]*:\/\//.test(text)) {
    try {
      const named = new URL(text);
      return named.host === host && !named.username && !named.password ? null : target.host;
    } catch {
      return target.host;
    }
  }
  // Otherwise the text must name this site and no other. Whole names only: "github.com" does not name "hub.com".
  const sites = text.match(/[a-z0-9-]+(?:\.[a-z0-9-]+)+(?::\d+)?/g) ?? [];
  return sites.length > 0 && sites.every((s) => s === host) ? null : target.host;
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
