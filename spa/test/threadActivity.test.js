// @vitest-environment jsdom
// Agent activity in the timeline: reasoning, tool calls, tool results,
// narration, and background tasks.
//
// A harness that reports its own work has no terminal to report it in, so the
// work goes into the conversation — the same timeline, the same events, the
// same cursor. What makes that safe to read is that activity is FOLDED: it
// arrives hundreds of items to a session, and the messages between them have to
// stay findable.

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
      { type: "event", data: { event: "task_update", summary: "started — run the full suite", created_at: "2026-08-23T12:00:04Z" } },
    ],
  });

describe("activity in the timeline", () => {
  it("renders the five kinds folded, and says which is which", () => {
    document.body.innerHTML = activity();

    const folds = [...document.querySelectorAll(".thread-activity")];
    expect(folds).toHaveLength(5);
    expect(folds.every((fold) => fold.tagName === "DETAILS")).toBe(true);
    // Folded: not one of them is open on arrival.
    expect(folds.every((fold) => fold.open)).toBe(false);
    expect(folds.map((fold) => fold.querySelector(".thread-activity-what").textContent)).toEqual([
      "Agent thought",
      "Agent called a tool",
      "Tool answered",
      "Agent narrated",
      "Background task",
    ]);
  });

  // Work the agent left running behind its own turn. It is the same quiet row
  // as the other four — the label is the whole difference — and it names the
  // task rather than the harness, because a background task is not the agent
  // speaking and the provider's name in front of it would say nothing.
  it("folds a background task shut under its own label, and shows what the task is", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "task_update", summary: "finished — run the full suite\n\n412 passed" } }],
      sessions: [{ provider: "claude" }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.tagName).toBe("DETAILS");
    expect(fold.open).toBe(false);
    expect(fold.querySelector(".thread-activity-what").textContent).toBe("Background task");
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("finished — run the full suite");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("412 passed");
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

// A tool call and its answer are ONE row: the call mints it, and the answer
// completes it in place. So the row has three states to say, and it says them
// with a mark on its own head rather than with a second row underneath.
describe("a tool call's own row", () => {
  const toolCall = (data) =>
    threadHtml({ items: [{ type: "event", data: { event: "tool_use", ...data } }] });

  // Absence is the pending state. It is what a call that has not been answered
  // yet carries, and also what every row written before calls and answers were
  // one row carries — so the unmarked row has to read exactly as it always did.
  it("carries no mark while the call is still running", () => {
    document.body.innerHTML = toolCall({ summary: "Read bridge/src/app.rs" });

    const fold = document.querySelector(".thread-activity");
    expect(fold.querySelector(".thread-activity-outcome")).toBe(null);
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
  });

  it("marks an answered call, and keeps the answer inside the fold", () => {
    document.body.innerHTML = toolCall({
      summary: "Read bridge/src/app.rs\n→ fn main() {}",
      outcome: "ok",
    });

    const fold = document.querySelector(".thread-activity");
    const mark = fold.querySelector(".thread-activity-outcome");
    expect(mark.dataset.outcome).toBe("ok");
    expect(mark.textContent).toBe("✓");
    // The head stays what it was: the answer suffix is a second line, so the
    // preview does not move when it lands.
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("→ fn main() {}");
  });

  // A failed tool call is still the agent working. It asks the reader for
  // nothing — the agent was told, and the agent calling the human is what a
  // blocker is for — so the colour is on the mark and the row stays toneless.
  it("colours the mark on a failed call, and nothing else about the row", () => {
    document.body.innerHTML = toolCall({
      summary: "Bash npm test\n→ 3 tests failed",
      outcome: "error",
    });

    const fold = document.querySelector(".thread-activity");
    const mark = fold.querySelector(".thread-activity-outcome");
    expect(mark.dataset.outcome).toBe("error");
    expect(mark.textContent).toBe("✕");
    expect(mark.classList.contains("blocked")).toBe(true);
    expect(fold.classList.contains("blocked")).toBe(false);
    expect(fold.querySelector(".thread-event-icon").textContent).toBe("▸");
  });

  // Pending is a claim too — "this is still running" — so a call nothing ever
  // answered closes rather than dangling, and the body names the boundary that
  // closed it.
  it("closes an unanswered call, and says which boundary closed it", () => {
    document.body.innerHTML = toolCall({
      summary: "Read bridge/src/app.rs\n→ no answer — turn ended",
      outcome: "unanswered",
    });

    const fold = document.querySelector(".thread-activity");
    const mark = fold.querySelector(".thread-activity-outcome");
    expect(mark.dataset.outcome).toBe("unanswered");
    expect(mark.textContent).toBe("⊘");
    expect(mark.classList.contains("blocked")).toBe(false);
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("no answer — turn ended");
  });

  // The wire is additive in the client's direction too: a daemon is free to
  // name a state this build predates, and the safe reading of a row whose state
  // this build cannot name is the one that claims nothing.
  it("renders an outcome token it has never heard of as pending", () => {
    document.body.innerHTML = toolCall({ summary: "Read bridge/src/app.rs", outcome: "deferred" });

    expect(document.querySelector(".thread-activity-outcome")).toBe(null);
  });

  // Stored rows must render forever, and the orphan fallback still mints them:
  // a `tool_result` row is its own row, with no mark and its own label.
  it("leaves a standalone tool_result row exactly as it was", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "tool_result", summary: "17 matches" } }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.querySelector(".thread-activity-what").textContent).toBe("Tool answered");
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("17 matches");
    expect(fold.querySelector(".thread-activity-outcome")).toBe(null);
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

  // The one repaint that changes the row under an open fold: the answer landing
  // on the call the reader opened to watch. The mark and the answer arrive, and
  // the fold stays open — otherwise watching a call run would shut the fold at
  // the exact moment it had something to show.
  it("stays open when the call's answer lands under it", () => {
    const pending = threadHtml({
      items: [{ type: "event", data: { sequence: 2, event: "tool_use", summary: "Bash npm test" } }],
    });
    const answered = threadHtml({
      items: [{
        type: "event",
        data: {
          sequence: 2,
          updated_sequence: 3,
          event: "tool_use",
          summary: "Bash npm test\n→ 412 passed",
          outcome: "ok",
        },
      }],
    });
    document.body.innerHTML = pending;
    const live = document.querySelector(".thread-items");
    live.querySelector(".thread-activity").open = true;

    const rendered = document.createElement("div");
    rendered.innerHTML = answered;
    patchElement(live, rendered.querySelector(".thread-items"));

    const fold = live.querySelector(".thread-activity");
    expect(fold.open).toBe(true);
    expect(fold.querySelector(".thread-activity-outcome").dataset.outcome).toBe("ok");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("412 passed");
  });
});
