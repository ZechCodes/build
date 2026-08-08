// @vitest-environment jsdom
// The "Working" line: once the agent has READ a user message, the thread says
// so under that message and counts up. It ends when the agent hands the turn
// back — an ordinary agent reply, or a terminal event — because a "Working"
// that outlives the work is worse than none.

import { describe, expect, it } from "vitest";
import { startWorkingTicker, threadHtml } from "../src/core/thread.js";

const userMsg = (over = {}) => ({
  type: "message",
  data: { id: "m1", role: "user", body: "do the thing", created_at: "2026-08-08T03:00:00Z", ...over },
});
const agentMsg = (over = {}) => ({
  type: "message",
  data: { id: "m2", role: "agent", body: "reply", created_at: "2026-08-08T03:05:00Z", ...over },
});
const html = (items) => threadHtml({ items });

describe("the Working line", () => {
  it("appears once the agent has read the message, and counts from when it read it", () => {
    const out = html([userMsg({ seen_at: "2026-08-08T03:01:00Z" })]);
    expect(out).toContain("thread-working");
    expect(out).toContain("Working");
    expect(out).toContain('data-since="2026-08-08T03:01:00Z"');
  });

  it("stays away until the agent has actually read it", () => {
    expect(html([userMsg()])).not.toContain("thread-working");
  });

  it("ends when the agent replies", () => {
    const out = html([userMsg({ seen_at: "2026-08-08T03:01:00Z" }), agentMsg()]);
    expect(out).not.toContain("thread-working");
  });

  it("survives a progress note — the agent said it is still going", () => {
    const out = html([userMsg({ seen_at: "2026-08-08T03:01:00Z" }), agentMsg({ still_working: true })]);
    expect(out).toContain("thread-working");
  });

  it("ends when the run reports done", () => {
    const out = html([
      userMsg({ seen_at: "2026-08-08T03:01:00Z" }),
      { type: "event", data: { kind: "done", summary: "finished" } },
    ]);
    expect(out).not.toContain("thread-working");
  });

  it("marks only the newest read message when several were sent", () => {
    const out = html([
      userMsg({ id: "m1", seen_at: "2026-08-08T03:01:00Z" }),
      userMsg({ id: "m2", seen_at: "2026-08-08T03:02:00Z" }),
    ]);
    expect(out.match(/class="thread-working"/g)).toHaveLength(1);
  });
});

describe("the Working ticker", () => {
  it("refreshes each counter in place from its own data-since", () => {
    document.body.innerHTML = threadHtml({
      items: [userMsg({ seen_at: "2026-08-08T03:00:00Z" })],
    });
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
