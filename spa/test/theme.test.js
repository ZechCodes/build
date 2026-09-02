// The light/dark/system preference: what is stored, how "system" resolves
// against the OS, and the segmented control the Account page renders.

import { describe, it, expect } from "vitest";
import {
  THEME_KEY,
  THEME_CHOICES,
  loadThemePreference,
  saveThemePreference,
  resolveTheme,
  themeControlHtml,
} from "../src/core/theme.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

describe("loadThemePreference", () => {
  it("follows the OS until the user says otherwise", () => {
    expect(loadThemePreference(memoryStorage())).toBe("system");
  });

  it("round-trips each choice", () => {
    for (const choice of THEME_CHOICES) {
      const storage = memoryStorage();
      saveThemePreference(choice, storage);
      expect(loadThemePreference(storage)).toBe(choice);
    }
  });

  it("reads an unknown or corrupt value as 'system'", () => {
    expect(loadThemePreference(memoryStorage({ [THEME_KEY]: "neon" }))).toBe("system");
    expect(loadThemePreference(memoryStorage({ [THEME_KEY]: "" }))).toBe("system");
  });

  it("survives a storage that throws (private mode)", () => {
    const hostile = refusingStorage();
    expect(loadThemePreference(hostile)).toBe("system");
    expect(() => saveThemePreference("dark", hostile)).not.toThrow();
  });
});

describe("saveThemePreference", () => {
  it("stores an unknown choice as 'system' rather than a value nothing can read", () => {
    const storage = memoryStorage();
    expect(saveThemePreference("chartreuse", storage)).toBe("system");
    expect(storage.getItem(THEME_KEY)).toBe("system");
  });
});

describe("resolveTheme", () => {
  it("hands 'system' to the OS", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });

  it("lets an explicit choice out-rank the OS", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });

  it("resolves an unknown preference the way 'system' would", () => {
    expect(resolveTheme("nonsense", true)).toBe("dark");
    expect(resolveTheme(undefined, false)).toBe("light");
  });
});

const segment = (html, choice) =>
  html.match(new RegExp(`<button[^>]*data-theme-choice="${choice}"[^>]*>`))?.[0] || "";

describe("themeControlHtml", () => {
  it("offers all three choices, marking the current one", () => {
    const html = themeControlHtml("dark");
    for (const choice of THEME_CHOICES) expect(segment(html, choice)).not.toBe("");
    expect(segment(html, "dark")).toContain("primary");
  });

  it("marks exactly one segment as chosen", () => {
    const html = themeControlHtml("light");
    expect(segment(html, "light")).toContain("primary");
    expect(segment(html, "dark")).not.toContain("primary");
    expect(segment(html, "system")).not.toContain("primary");
  });

  it("falls back to 'system' when the stored preference is unreadable", () => {
    expect(segment(themeControlHtml("nonsense"), "system")).toContain("primary");
  });
});
