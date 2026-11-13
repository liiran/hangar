import { describe, expect, it } from "vitest";
import { hiddenParts, linkHost, rightToLeft, ruleText, sessionRules, visible, webUrl } from "./safe";

describe("safe", () => {
  it("writes out invisible and direction-changing characters", () => {
    expect(visible("rm -rf ~ \u202Egnp.txt")).toBe("rm -rf ~ ⟨U+202E⟩gnp.txt");
    expect(visible("a\u200Bb\u00A0c\u0007d\u001B[31m")).toBe("a⟨U+200B⟩b⟨U+00A0⟩c⟨U+0007⟩d⟨U+001B⟩[31m");
    expect(visible("tag\u{E0041}")).toBe("tag⟨U+E0041⟩");
    expect(hiddenParts("ok\u2066x").map((p) => p.hidden)).toEqual([false, true, false]);
    // Blank characters that count as letters, so a diff could add a variable nobody sees.
    expect(visible("let \u3164 = steal()")).toBe("let ⟨U+3164⟩ = steal()");
    expect(visible("a\u115Fb\uFFA0c\u2800d\u3000e")).toBe("a⟨U+115F⟩b⟨U+FFA0⟩c⟨U+2800⟩d⟨U+3000⟩e");
    // Selectors that change nothing visible, so two names can look the same.
    expect(visible("file\uFE0F.txt\u{E0100}")).toBe("file⟨U+FE0F⟩.txt⟨U+E0100⟩");
  });

  it("notices right-to-left letters", () => {
    expect(rightToLeft("rm \u05D0\u05D1 123")).toBe(true);
    expect(rightToLeft("\u0645\u0631\u062D\u0628\u0627")).toBe(true);
    expect(rightToLeft("plain text, café, 日本語")).toBe(false);
  });

  it("leaves ordinary text, tabs and line ends alone", () => {
    const text = "line one\n\tindented — café 日本語 😀\r\nline three";
    expect(visible(text)).toBe("line one\n\tindented — café 日本語 😀\nline three");
    expect(visible("lone\rreturn")).toBe("lone⟨U+000D⟩return");
    expect(hiddenParts("")).toEqual([]);
  });

  it("only lets absolute web addresses become links", () => {
    expect(webUrl("https://example.com/a?b=1")).toBe("https://example.com/a?b=1");
    expect(webUrl("HTTP://Example.com")).toBe("http://example.com/");
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "vbscript:x", "file:///C:/x", "mailto:a@b.c", "/relative", "#pair=abc", "//evil.example/x", "", " ", null, undefined, 5, "https://", "ms-settings:x"]) {
      expect(webUrl(bad), String(bad)).toBeNull();
    }
  });

  it("names the real host when the link text does not", () => {
    expect(linkHost("https://evil.example/x", "https://github.com/org/repo")).toBe("evil.example");
    expect(linkHost("https://evil.example/x", "click here")).toBe("evil.example");
    expect(linkHost("https://github.com/org/repo", "https://github.com/org/repo")).toBeNull();
    expect(linkHost("https://docs.rs/tokio", "the docs.rs page")).toBeNull();
    expect(linkHost("https://github.com.evil.example/x", "github.com")).toBe("github.com.evil.example");
    // A site whose name is part of the one shown.
    expect(linkHost("https://hub.com/x", "github.com/org/repo")).toBe("hub.com");
    expect(linkHost("https://hub.com/x", "see github.com")).toBe("hub.com");
    // A name before "@" is not the site, even when the text is the address itself.
    expect(linkHost("https://github.com@evil.example/login", "https://github.com@evil.example/login")).toBe("evil.example");
    expect(linkHost("https://evil.example/", "https://github.com:x@evil.example/")).toBe("evil.example");
    // Text that names two sites.
    expect(linkHost("https://evil.example/x", "evil.example mirrors github.com")).toBe("evil.example");
    expect(linkHost("https://GitHub.com/org", "GitHub.com")).toBeNull();
  });

  it("writes session rules the way Claude Code does", () => {
    expect(ruleText({ toolName: "Bash", ruleContent: "npm test:*" })).toBe("Bash(npm test:*)");
    expect(ruleText({ toolName: "WebFetch" })).toBe("WebFetch (every use)");
    expect(ruleText({ ruleContent: "x" })).toBeNull();
    expect(ruleText(null)).toBeNull();
    const suggestions = [
      { type: "addRules", behavior: "allow", destination: "localSettings", rules: [{ toolName: "Bash", ruleContent: "python:*" }, { toolName: "Read" }] },
      { type: "setMode", mode: "bypassPermissions" },
      { type: "addRules", behavior: "deny", rules: [{ toolName: "Bash", ruleContent: "rm" }] },
      { type: "addDirectories", directories: ["C:/"] },
    ];
    expect(sessionRules(suggestions)).toEqual(["Bash(python:*)", "Read (every use)"]);
    expect(sessionRules(null)).toEqual([]);
  });
});
