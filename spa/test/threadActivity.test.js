// @vitest-environment jsdom
// Agent activity in the timeline: reasoning, tool calls, tool results and
// narration.
//
// A harness that reports its own work has no terminal to report it in, so the
// work goes into the conversation — the same timeline, the same events, the
// same cursor. What makes that safe to read is that activity is FOLDED: it
// arrives hundreds of items to a session, and the four messages between them
// have to stay findable.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";
import { patchElement } from "../src/core/domPatch.js";

const activity = () =>
  threadHtml({
    items: [
      { type: "event", data: { event: "reasoning", summary: "The parser is re-entrant, so the lock has to move.", created_at: "2026-08-23T12:00:00Z" } },
      { type: "event", data: { event: "tool_use", summary: "Read bridge/src/app.rs", created_at: "2026-08-23T12:00:01Z" } },
      { type: "event", data: { event: "tool_result", summary: "17 matches", created_at: "2026-08-23T12:00:02Z" } },
      { type: "event", data: { event: "narration", summary: "Running the suite once more.", created_at: "2026-08-23T12:00:03Z" } },
    ],
  });

describe("activity in the timeline", () => {
  it("renders the four kinds folded, and says which is which", () => {
    document.body.innerHTML = activity();

    const folds = [...document.querySelectorAll(".thread-activity")];
    expect(folds).toHaveLength(4);
    expect(folds.every((fold) => fold.tagName === "DETAILS")).toBe(true);
    // Folded: not one of them is open on arrival.
    expect(folds.every((fold) => fold.open)).toBe(false);
    expect(folds.map((fold) => fold.querySelector(".thread-activity-what").textContent)).toEqual([
      "Agent thought",
      "Agent called a tool",
      "Tool answered",
      "Agent narrated",
    ]);
  });

  // The fold's head is what a reader scans past. A stack of rows all saying
  // "Agent called a tool" says nothing; the first line of what the agent
  // actually did is the whole value of the row.
  it("carries the summary's first line in the fold's head, and the whole of it inside", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "tool_use", summary: "Read bridge/src/app.rs\n\nlines 7601-7640" } }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("lines 7601-7640");
  });

  // Quieter than a message: activity is not somebody speaking, so it keeps the
  // event's own dim row rather than a message's card, avatar and head.
  it("is quieter than a message", () => {
    document.body.innerHTML = activity();

    expect(document.querySelectorAll(".thread-comment")).toHaveLength(0);
    expect(document.querySelectorAll(".thread-avatar")).toHaveLength(0);
    expect([...document.querySelectorAll(".thread-activity")].every((fold) => fold.classList.contains("thread-event"))).toBe(true);
  });

  // Nothing to open is not a fold. An activity event with neither a summary nor
  // links would otherwise offer a disclosure triangle onto an empty box.
  it("does not offer a fold with nothing behind it", () => {
    document.body.innerHTML = threadHtml({ items: [{ type: "event", data: { event: "reasoning" } }] });

    const row = document.querySelector(".thread-activity");
    expect(row.tagName).toBe("DIV");
    expect(row.textContent).toContain("Agent thought");
  });

  it("names the harness the way every other event does", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "reasoning", summary: "Checking the lock order." } }],
      sessions: [{ provider: "claude" }],
    });

    expect(document.querySelector(".thread-activity-what").textContent).toBe("Claude Code thought");
  });

  // Everything else on the thread is untouched. The four kinds are the only
  // ones that fold; a kind this client has never heard of still renders as the
  // plain row it always did, rather than being swept in with them.
  it("leaves every other event exactly as it was", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "done", summary: "Implemented the requested change." } },
        { type: "event", data: { event: "some_new_kind", summary: "Something happened." } },
      ],
    });

    expect(document.querySelectorAll(".thread-activity")).toHaveLength(0);
    expect(document.querySelectorAll("details")).toHaveLength(0);
    const events = [...document.querySelectorAll(".thread-event")];
    expect(events).toHaveLength(2);
    expect(events[0].textContent).toContain("Agent reported done");
    expect(events[1].textContent).toContain("some new kind");
    expect(events[1].textContent).toContain("Something happened.");
  });
});

// The conversation is re-rendered on every poll and patched into the live tree.
// The render always ships a fold shut, so `open` is on the live element only
// because the reader put it there — and a patch that took it back would shut
// every fold the reader opened, once every 1.6 seconds.
describe("a fold the reader opened", () => {
  it("survives the repaint under it", () => {
    document.body.innerHTML = activity();
    const live = document.querySelector(".thread-items");
    live.querySelector(".thread-activity").open = true;

    const rendered = document.createElement("div");
    rendered.innerHTML = activity();
    patchElement(live, rendered.querySelector(".thread-items"));

    expect(live.querySelector(".thread-activity").open).toBe(true);
  });
});
