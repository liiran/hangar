import { describe, expect, it } from "vitest";
import { basename, bytes, duration, fillColor, GROUP_ORDER, limitName, modelChoice, resetTime, STATUS, tokens } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(bytes(null)).toBe("—");
    expect(bytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(bytes(450 * 1024 * 1024)).toBe("450 MB");
    expect(bytes(1.5 * 1024 ** 3)).toBe("1.5 GB");
    expect(bytes(31 * 1024 ** 3)).toBe("31 GB");
  });

  it("formats durations", () => {
    expect(duration(5)).toBe("5 ms");
    expect(duration(1500)).toBe("1.5 s");
    expect(duration(42_000)).toBe("42 s");
    expect(duration(125_000)).toBe("2m 5s");
  });

  it("takes the last path segment on both separators", () => {
    expect(basename("C:\\code\\my-app\\")).toBe("my-app");
    expect(basename("/home/me/proj")).toBe("proj");
  });

  it("maps model names to the model picker's choices", () => {
    expect(modelChoice(null)).toBe("");
    expect(modelChoice("haiku")).toBe("haiku");
    expect(modelChoice("claude-sonnet-5-5")).toBe("sonnet");
    expect(modelChoice("claude-opus-5-5[1m]")).toBe("opus");
    expect(modelChoice("some-other-model")).toBe("");
  });

  it("writes token counts the way Claude Code does", () => {
    expect([0, 950, 1000, 28_884, 84_000, 200_000, 967_000].map(tokens)).toEqual(["0", "950", "1k", "28.9k", "84k", "200k", "967k"]);
    expect([999_949, 999_950, 1_000_000, 1_250_000].map(tokens)).toEqual(["999.9k", "1M", "1M", "1.3M"]);
  });

  it("warms the fill color in steps: calm, yellow from half, orange from 70%, deeper from 85%, red from 95%", () => {
    const steps = [0, 49, 50, 69, 70, 84, 85, 94, 95, 100, 140].map(fillColor);
    expect(steps).toEqual(["var(--accent)", "var(--accent)", "var(--fill-1)", "var(--fill-1)", "var(--fill-2)", "var(--fill-2)", "var(--fill-3)", "var(--fill-3)", "var(--fill-4)", "var(--fill-4)", "var(--fill-4)"]);
  });

  it("names the usage windows and says when they start over", () => {
    expect(["five_hour", "seven_day", "seven_day_opus", "weekly"].map(limitName)).toEqual(["5-hour limit", "7-day limit", "7-day Opus limit", "weekly limit"]);
    const now = new Date(2026, 10, 15, 13, 0).getTime();
    const today = resetTime(new Date(2026, 10, 15, 15, 40).getTime() / 1000, now);
    const later = resetTime(new Date(2026, 10, 18, 14, 0).getTime() / 1000, now);
    // The time is in the user's own format; the weekday is there only on another day.
    expect(today).toMatch(/40/);
    expect(later.endsWith(resetTime(new Date(2026, 10, 15, 14, 0).getTime() / 1000, now))).toBe(true);
    expect(later.length).toBeGreaterThan(today.length);
  });

  it("puts every status in a known group", () => {
    for (const s of Object.values(STATUS)) expect(GROUP_ORDER).toContain(s.group);
  });
});
