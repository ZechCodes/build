// @vitest-environment jsdom
// Working time is branch/issue-level status: it belongs to the toolbar, where
// it is true wherever you are standing in the work, and not to the conversation
// — a timeline that narrates the clock buries what was actually said.
//
// The ticker outlives that decision only for the pre-redesign plan surface,
// which still paints its own line; it refreshes whatever it is given.

import { describe, expect, it } from "vitest";
import { startWorkingTicker, threadHtml } from "../src/core/thread.js";

const userMsg = (over = {}) => ({
  type: "message",
  data: { id: "m1", role: "user", body: "do the thing", created_at: "2026-08-08T03:00:00Z", ...over },
});

describe("the conversation and the clock", () => {
  it("says nothing about working time, however long the agent has had the message", () => {
    const out = threadHtml({ items: [userMsg({ seen_at: "2026-08-08T03:01:00Z" })] });
    expect(out).not.toContain("thread-working");
    expect(out).toContain("Seen");
  });
});

describe("the Working ticker", () => {
  const line = (since) =>
    `<div class="thread-working" data-since="${since}"><span class="thread-working-age">0s</span></div>`;

  it("refreshes each counter in place from its own data-since", () => {
    document.body.innerHTML = line("2026-08-08T03:00:00Z");
    let fire = null;
    const stop = startWorkingTicker(document.body, {
      setIntervalImpl: (fn) => { fire = fn; return 1; },
      clearIntervalImpl: () => { fire = null; },
      now: () => Date.parse("2026-08-08T03:02:30Z"),
    });
    fire();
    expect(document.querySelector(".thread-working-age").textContent).toBe("2m");
    stop();
    expect(fire).toBe(null);
  });

  it("retires itself once its surface is gone", () => {
    const root = document.createElement("div"); // never attached
    let fire = null, cleared = false;
    startWorkingTicker(root, {
      setIntervalImpl: (fn) => { fire = fn; return 7; },
      clearIntervalImpl: () => { cleared = true; },
    });
    fire();
    expect(cleared).toBe(true);
  });
});
