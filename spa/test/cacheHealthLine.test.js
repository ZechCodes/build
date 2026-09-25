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

  it("says a write was not kept because storage was full", () => {
    expect(cacheHealthText({ state: "ready", lastRecovery: null, lastRefused: { at: AT, error: "QuotaExceededError" } }))
      .toBe("Working. At 13:43:02 a write was not kept because this browser's storage for Build was full.");
  });

  it("says nothing of a refused write that was not about space", () => {
    expect(cacheHealthText({ state: "ready", lastRecovery: null, lastRefused: { at: AT, error: "DataCloneError" } }))
      .toBe("Working.");
  });

  it("says it is reconnecting and since when, without the browser's message", () => {
    // WebKit's message ends "Refresh the page to try again", which is exactly
    // what nobody should have to do; it stays in the Diagnostics dump.
    expect(cacheHealthText({
      state: "recovering", since: AT, error: "UnknownError",
      message: "Connection to Indexed Database server lost. Refresh the page to try again",
    })).toBe("Reconnecting since 13:43:02.");
  });

  it("says it is waiting to try again", () => {
    expect(cacheHealthText({ state: "resting", since: AT, error: "UnknownError", message: "" }))
      .toBe("Not answering since 13:43:02. Trying again when the app is next opened, or in a moment.");
  });

  const stoodDown = (reason, error, message = "") => cacheHealthText({ state: "stood-down", at: AT, reason, error, message });

  it("says it stood down after failing to open for too long, and that a reload tries again", () => {
    expect(stoodDown("persistent", "UnknownError", "Error creating or migrating Records table in database"))
      .toBe("Off for this session since 13:43:02: it kept failing to open. Reload to try again.");
  });

  it("says a private window refused it, without promising a reload helps", () => {
    for (const error of ["InvalidStateError", "SecurityError"]) {
      expect(stoodDown("open-failed", error, "The operation is insecure."))
        .toBe("Off for this session since 13:43:02: this browser refused to open it, as a private window does. Nothing is kept between visits.");
    }
  });

  it("says a newer version changed it", () => {
    expect(stoodDown("open-failed", "VersionError"))
      .toBe("Off for this session since 13:43:02: a newer version of Build has changed it. Reload to use that version.");
  });

  it("says the browser refused it for any other reason", () => {
    expect(stoodDown("open-failed", "NotFoundError")).toBe("Off for this session since 13:43:02: this browser refused to open it. Reload to try again.");
    expect(stoodDown("transaction-failed", "NotFoundError")).toBe("Off for this session since 13:43:02: this browser refused to use it. Reload to try again.");
  });

  it("says it is waiting for another tab", () => {
    expect(cacheHealthText({ state: "blocked", since: AT }))
      .toBe("Waiting since 13:43:02 for another tab with an older version of Build to close.");
  });

  it("says another tab kept it closed", () => {
    expect(cacheHealthText({ state: "stood-down", at: AT, reason: "blocked", error: "BlockedError", message: "" }))
      .toBe("Off for this session since 13:43:02: another tab with an older version of Build kept it closed. Close that tab and reload to turn it back on.");
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
    expect(text()).toBe("Reconnecting since 13:43:02.");

    dispose();
    health = { state: "absent" };
    vi.advanceTimersByTime(5000);
    expect(text()).toBe("Reconnecting since 13:43:02.");
  });

  it("does nothing on a page without the line", () => {
    expect(mountCacheHealthLine(document.body, { health: () => ({ state: "ready" }) })).toBeTypeOf("function");
  });
});
