// @vitest-environment jsdom
// What the theme actually does to the page: the resolved theme is stamped on
// <html> (that attribute is the only thing styles.css keys off), the browser
// chrome colour follows it, and "system" keeps tracking the OS after boot.

import { describe, it, expect, beforeEach } from "vitest";
import {
  THEME_KEY,
  applyTheme,
  applyStoredTheme,
  installTheme,
  bindThemeControl,
  themeControlHtml,
  terminalTheme,
} from "../src/core/theme.js";
import { memoryStorage } from "./memoryStorage.js";

// A matchMedia stand-in whose match state can be flipped, notifying listeners
// the way the browser does when the OS switches appearance.
const fakeMedia = (matches = false) => {
  const listeners = new Set();
  return {
    matches,
    addEventListener: (_type, fn) => listeners.add(fn),
    removeEventListener: (_type, fn) => listeners.delete(fn),
    flip(next) {
      this.matches = next;
      listeners.forEach((fn) => fn({ matches: next }));
    },
  };
};

describe("applyTheme", () => {
  beforeEach(() => {
    document.documentElement.removeAttribute("data-theme");
    document.head.innerHTML = "";
  });

  it("stamps the resolved theme on the document element", () => {
    applyTheme("dark", document);
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    applyTheme("light", document);
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
  });

  it("moves the browser chrome colour with the theme", () => {
    document.head.innerHTML = '<meta name="theme-color" content="#5b62e8" />';
    applyTheme("dark", document);
    const dark = document.querySelector('meta[name="theme-color"]').content;
    applyTheme("light", document);
    expect(document.querySelector('meta[name="theme-color"]').content).not.toBe(dark);
  });

  it("keeps every media-specific browser chrome meta in sync with an explicit choice", () => {
    document.head.innerHTML = `
      <meta name="theme-color" media="(prefers-color-scheme: light)" content="#fff" />
      <meta name="theme-color" media="(prefers-color-scheme: dark)" content="#000" />`;
    applyTheme("dark");
    expect([...document.querySelectorAll('meta[name="theme-color"]')].map((meta) => meta.content)).toEqual([
      "#07151b",
      "#07151b",
    ]);
  });

  it("works on a document with no theme-color meta", () => {
    expect(() => applyTheme("dark", document)).not.toThrow();
  });
});

describe("applyStoredTheme", () => {
  beforeEach(() => document.documentElement.removeAttribute("data-theme"));

  it("resolves the stored preference against the OS", () => {
    const storage = memoryStorage({ [THEME_KEY]: "system" });
    expect(applyStoredTheme({ doc: document, storage, media: fakeMedia(true) })).toBe("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("honours an explicit choice against a disagreeing OS", () => {
    const storage = memoryStorage({ [THEME_KEY]: "light" });
    expect(applyStoredTheme({ doc: document, storage, media: fakeMedia(true) })).toBe("light");
  });

  it("falls back to light when the browser cannot report a preference", () => {
    expect(applyStoredTheme({ doc: document, storage: memoryStorage(), media: null })).toBe("light");
  });
});

describe("installTheme", () => {
  beforeEach(() => document.documentElement.removeAttribute("data-theme"));

  it("keeps following the OS while the preference is 'system'", () => {
    const media = fakeMedia(false);
    installTheme({ doc: document, storage: memoryStorage(), media });
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    media.flip(true);
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("ignores the OS once the user has chosen a side", () => {
    const media = fakeMedia(false);
    installTheme({ doc: document, storage: memoryStorage({ [THEME_KEY]: "light" }), media });
    media.flip(true);
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
  });
});

describe("terminalTheme", () => {
  beforeEach(() => {
    document.documentElement.style.removeProperty("--term-bg");
    document.documentElement.style.removeProperty("--term-fg");
  });

  it("takes the terminal's colours from the page's own tokens", () => {
    document.documentElement.style.setProperty("--term-bg", "#15171c");
    document.documentElement.style.setProperty("--term-fg", "#c3cae0");
    expect(terminalTheme(document)).toEqual({ background: "#15171c", foreground: "#c3cae0" });
  });

  it("still yields a dark terminal where the tokens cannot be read", () => {
    const { background, foreground } = terminalTheme(document);
    expect(background).toMatch(/^#[0-9a-f]{6}$/);
    expect(foreground).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("bindThemeControl", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    document.documentElement.removeAttribute("data-theme");
  });

  it("stores the clicked choice and repaints the page and the control", () => {
    const storage = memoryStorage();
    const media = fakeMedia(false);
    const host = document.createElement("div");
    host.innerHTML = themeControlHtml("system");
    document.body.append(host);
    bindThemeControl(host, { doc: document, storage, media });

    host.querySelector('[data-theme-choice="dark"]').click();
    expect(storage.getItem(THEME_KEY)).toBe("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(host.querySelector('[data-theme-choice="dark"]').className).toContain("primary");
    expect(host.querySelector('[data-theme-choice="system"]').className).not.toContain("primary");
  });

  it("returns to the OS when 'system' is picked back", () => {
    const storage = memoryStorage({ [THEME_KEY]: "light" });
    const media = fakeMedia(true);
    const host = document.createElement("div");
    host.innerHTML = themeControlHtml("light");
    document.body.append(host);
    bindThemeControl(host, { doc: document, storage, media });

    host.querySelector('[data-theme-choice="system"]').click();
    expect(storage.getItem(THEME_KEY)).toBe("system");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });
});
