// @vitest-environment jsdom
// The local cache's state, in Settings beside the build line (#169). A phone
// has no console: this line and the Diagnostics dump above it are how somebody
// holding one tells "the cache stood down" from "the bridge sent nothing".

import { afterEach, describe, expect, it, vi } from "vitest";
import { cacheHealthLineHtml, cacheHealthText, mountCacheHealthLine } from "../src/core/cacheHealthLine.js";

const AT = new Date(2026, 8, 25, 13, 43, 2).getTime();

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("what the line says", () => {
  it("says the cache is working", () => {
    expect(cacheHealthText({ state: "ready", lastRecovery: null })).toBe("Working.");
  });

  it("says when it last came back, and how long it was away", () => {
    expect(cacheHealthText({ state: "ready", lastRecovery: { at: AT, afterMs: 1250, attempts: 3 } }))
      .toBe("Working. Reconnected at 13:43:02 after 1.3 s.");
  });

  it("says it is reconnecting, since when, and the error", () => {
    expect(cacheHealthText({ state: "recovering", since: AT, error: "UnknownError", message: "Connection to Indexed Database server lost" }))
      .toBe("Reconnecting since 13:43:02. UnknownError: Connection to Indexed Database server lost");
  });

  it("says it is waiting to try again", () => {
    expect(cacheHealthText({ state: "resting", since: AT, error: "UnknownError", message: "" }))
      .toBe("Not answering since 13:43:02. Trying again when the app is next opened, or in a moment. UnknownError");
  });

  it("says it stood down, why, and what brings it back", () => {
    expect(cacheHealthText({ state: "stood-down", at: AT, error: "QuotaExceededError", message: "The quota has been exceeded." }))
      .toBe("Off for this session since 13:43:02. QuotaExceededError: The quota has been exceeded. Reload to turn it back on.");
  });

  it("says when the browser has none", () => {
    expect(cacheHealthText({ state: "absent" })).toBe("This browser has no IndexedDB, so nothing is kept between visits.");
  });
});

describe("the line on the page", () => {
  it("paints the state now and keeps it current until disposed", () => {
    vi.useFakeTimers();
    let health = { state: "ready", lastRecovery: null };
    document.body.innerHTML = `<div id="host">${cacheHealthLineHtml()}</div>`;
    const dispose = mountCacheHealthLine(document.querySelector("#host"), { health: () => health, pollMs: 1000 });
    const text = () => document.querySelector("#cachehealthtext").textContent;
    expect(document.querySelector("#cachehealth").textContent).toContain("Local cache");
    expect(text()).toBe("Working.");

    health = { state: "recovering", since: AT, error: "UnknownError", message: "" };
    vi.advanceTimersByTime(1000);
    expect(text()).toBe("Reconnecting since 13:43:02. UnknownError");

    dispose();
    health = { state: "absent" };
    vi.advanceTimersByTime(5000);
    expect(text()).toBe("Reconnecting since 13:43:02. UnknownError");
  });

  it("does nothing on a page without the line", () => {
    expect(mountCacheHealthLine(document.body, { health: () => ({ state: "ready" }) })).toBeTypeOf("function");
  });
});
