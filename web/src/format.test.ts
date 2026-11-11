import { describe, expect, it } from "vitest";
import { basename, bytes, duration, GROUP_ORDER, modelChoice, STATUS } from "./format";

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

  it("puts every status in a known group", () => {
    for (const s of Object.values(STATUS)) expect(GROUP_ORDER).toContain(s.group);
  });
});
