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
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { threadHtml } from "../src/core/thread.js";
import { patchElement } from "../src/core/domPatch.js";

const ACTIVITY_STYLES = readFileSync(resolve("src/styles.css"), "utf8");

// A run draws what it stands for only when the reader has it open — a shut one
// is a head (core/thread.js `activityRunHtml`), and which runs are open is the
// pane's to say. Where a test is about the rows INSIDE a run, it renders with
// every run open.
const EVERY_RUN_OPEN = { has: () => true };

const openThreadHtml = (thread) => threadHtml(thread, { openRuns: EVERY_RUN_OPEN });

const ACTIVITY_ITEMS = [
  { type: "event", data: { event: "reasoning", summary: "The parser is re-entrant, so the lock has to move.", created_at: "2026-08-23T12:00:00Z" } },
  { type: "event", data: { event: "tool_use", summary: "Read bridge/src/app.rs", created_at: "2026-08-23T12:00:01Z" } },
  { type: "event", data: { event: "tool_result", summary: "17 matches", created_at: "2026-08-23T12:00:02Z" } },
  { type: "event", data: { event: "narration", summary: "Running the suite once more.", created_at: "2026-08-23T12:00:03Z" } },
  { type: "event", data: { event: "task_update", summary: "started — run the full suite", created_at: "2026-08-23T12:00:04Z" } },
];

const activity = () => threadHtml({ items: ACTIVITY_ITEMS });

const openActivity = () => openThreadHtml({ items: ACTIVITY_ITEMS });

describe("activity in the timeline", () => {
  // A row is its content. The kind is not spent on the line — the icon carries
  // it, and carries it as an `aria-label` so a reader who cannot see the icon
  // still hears which kind the row is.
  it("renders the five kinds folded, and says which is which on the icon alone", () => {
    document.body.innerHTML = openActivity();

    const folds = [...document.querySelectorAll(".thread-activity")];
    expect(folds).toHaveLength(5);
    expect(folds.every((fold) => fold.tagName === "DETAILS")).toBe(true);
    // Folded: not one of them is open on arrival.
    expect(folds.every((fold) => fold.open)).toBe(false);
    expect(document.querySelectorAll(".thread-activity-what")).toHaveLength(0);
    expect(folds.map((fold) => fold.querySelector(".thread-event-icon").getAttribute("aria-label"))).toEqual([
      "Agent thought",
      "Agent called a tool",
      "Tool answered",
      "Agent narrated",
      "Background task",
    ]);
    // The line itself is the content, in full — no label eating the front of it.
    expect(folds.map((fold) => fold.querySelector(".thread-activity-preview").textContent)).toEqual([
      "The parser is re-entrant, so the lock has to move.",
      "Read bridge/src/app.rs",
      "17 matches",
      "Running the suite once more.",
      "started — run the full suite",
    ]);
  });

  // Work the agent left running behind its own turn. It is the same quiet row
  // as the other four, and it shows the task rather than the harness: a
  // background task is not the agent speaking, and the provider's name in front
  // of it would say nothing.
  it("folds a background task shut, and gives the line to what the task is", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "task_update", summary: "finished — run the full suite\n\n412 passed" } }],
      sessions: [{ provider: "claude_adk" }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.tagName).toBe("DETAILS");
    expect(fold.open).toBe(false);
    expect(fold.querySelector(".thread-activity-what")).toBe(null);
    expect(fold.querySelector(".thread-event-icon").getAttribute("aria-label")).toBe("Background task");
    // Minted before the daemon led with the description, and rendered as it was
    // minted: a persisted summary is a record, not a template.
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("finished — run the full suite");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("412 passed");
  });

  // The summary is a record minted by the daemon, and the client renders it as
  // it was minted. A row written before the daemon knew how to summarise a call
  // reads exactly as it was stored — no display-time prettifying, which would
  // be a second half-copy of the mint that has to agree with it forever.
  it("renders a row minted by an older daemon verbatim", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "tool_use", summary: 'Tool {"a":1}' } }],
    });

    expect(document.querySelector(".thread-activity-preview").textContent).toBe('Tool {"a":1}');
  });

  // The fold's head is what a reader scans past. A stack of rows all saying
  // "Agent called a tool" says nothing; the first line of what the agent
  // actually did is the whole value of the row.
  it("carries the summary's first line in the fold's head, and the whole of it inside", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "tool_use", summary: "Read bridge/src/app.rs\n\nlines 7601-7640" } }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
    expect(fold.querySelector(".thread-event-detail").textContent).toContain("lines 7601-7640");
  });

  // Quieter than a message: activity is not somebody speaking, so it keeps the
  // event's own dim row rather than a message's card, avatar and head.
  it("is quieter than a message", () => {
    document.body.innerHTML = openActivity();

    expect(document.querySelectorAll(".thread-comment")).toHaveLength(0);
    expect(document.querySelectorAll(".thread-avatar")).toHaveLength(0);
    expect([...document.querySelectorAll(".thread-activity")].every((fold) => fold.classList.contains("thread-event"))).toBe(true);
  });

  // Nothing to open is not a fold. An activity event with neither a summary nor
  // links would otherwise offer a disclosure triangle onto an empty box.
  //
  // It is also the one row with no content of its own, so the kind is what it
  // shows: a blank line is worse than the label.
  it("does not offer a fold with nothing behind it, and shows the kind in place of the line", () => {
    document.body.innerHTML = openThreadHtml({ items: [{ type: "event", data: { event: "reasoning" } }] });

    const row = document.querySelector(".thread-activity");
    expect(row.tagName).toBe("DIV");
    expect(row.querySelector(".thread-activity-preview").textContent).toBe("Agent thought");
  });

  it("names the harness the way every other event does", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "reasoning" } }],
      sessions: [{ provider: "claude_adk" }],
    });

    const row = document.querySelector(".thread-activity");
    expect(row.querySelector(".thread-activity-preview").textContent).toBe("Claude Code thought");
    expect(row.querySelector(".thread-event-icon").getAttribute("aria-label")).toBe("Claude Code thought");
  });

  // Everything else on the thread is untouched. The four kinds are the only
  // ones that fold; a kind this client has never heard of still renders as the
  // plain row it always did, rather than being swept in with them.
  it("leaves every other event exactly as it was", () => {
    document.body.innerHTML = openThreadHtml({
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
    openThreadHtml({ items: [{ type: "event", data: { event: "tool_use", ...data } }] });

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
  // a `tool_result` row is its own row, with no mark and its own kind.
  it("leaves a standalone tool_result row exactly as it was", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "tool_result", summary: "17 matches" } }],
    });

    const fold = document.querySelector(".thread-activity");
    expect(fold.querySelector(".thread-event-icon").getAttribute("aria-label")).toBe("Tool answered");
    expect(fold.querySelector(".thread-activity-preview").textContent).toBe("17 matches");
    expect(fold.querySelector(".thread-activity-outcome")).toBe(null);
  });

  // How a call ended is a fact about what the call did, so it sits with the
  // content — and the timestamp goes back to being the line's quiet right edge,
  // on the row and on the run's line alike.
  it("puts the mark on the line after the content, and the time last", () => {
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "tool_use", summary: "Bash cargo test", outcome: "ok", created_at: "2026-08-23T12:00:00Z" } }],
    });

    const order = (head) => [...head.children].map((node) => node.className || node.tagName.toLowerCase());
    const rowHead = document.querySelector(".thread-activity .thread-activity-head");
    const runHead = document.querySelector(".thread-activity-group-head");
    for (const head of [rowHead, runHead]) {
      const classes = order(head);
      expect(classes.filter((name) => name.includes("outcome"))).toHaveLength(1);
      expect(classes.findIndex((name) => name.includes("outcome")))
        .toBeGreaterThan(classes.findIndex((name) => name.includes("preview")));
      expect(classes.findIndex((name) => name === "time"))
        .toBeGreaterThan(classes.findIndex((name) => name.includes("outcome")));
    }
  });
});

// The conversation is re-rendered on every poll and patched into the live tree.
// The render always ships a fold shut, so `open` is on the live element only
// because the reader put it there — and a patch that took it back would shut
// every fold the reader opened, once every 1.6 seconds.
describe("a fold the reader opened", () => {
  it("survives the repaint under it", () => {
    document.body.innerHTML = openActivity();
    const live = document.querySelector(".thread-items");
    live.querySelector(".thread-activity").open = true;

    const rendered = document.createElement("div");
    rendered.innerHTML = openActivity();
    patchElement(live, rendered.querySelector(".thread-items"));

    expect(live.querySelector(".thread-activity").open).toBe(true);
  });

  // The one repaint that changes the row under an open fold: the answer landing
  // on the call the reader opened to watch. The mark and the answer arrive, and
  // the fold stays open — otherwise watching a call run would shut the fold at
  // the exact moment it had something to show.
  it("stays open when the call's answer lands under it", () => {
    const pending = openThreadHtml({
      items: [{ type: "event", data: { sequence: 2, event: "tool_use", summary: "Bash npm test" } }],
    });
    const answered = openThreadHtml({
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

// A run of activity is one line.
//
// Folding each item on its own was the right answer to a row nobody can scan,
// and the wrong answer to two hundred of them: a message with forty tool calls
// under it is a message the reader has to scroll for. So between two things
// somebody SAID, the whole run collapses to a single dim line — how many, and
// the newest one's own words with no label in front of them — and opens onto
// the rows themselves only when the reader asks.
describe("a run of activity between messages", () => {
  const event = (data) => ({ type: "event", data });
  const message = (body) => ({ type: "message", data: { role: "agent", body } });
  const groups = () => [...document.querySelectorAll(".thread-activity-group")];
  const timeline = () => [...document.querySelector(".thread-items").children];

  it("collapses the whole run into one line: how many rows, and the last tool called", () => {
    document.body.innerHTML = activity();

    expect(groups()).toHaveLength(1);
    expect(timeline()).toHaveLength(1);
    const group = groups()[0];
    expect(group.tagName).toBe("DETAILS");
    // Shut on arrival: activity asks the reader for nothing.
    expect(group.open).toBe(false);
    // Five rows, one tool call. The number is how much is inside the fold —
    // every row, thought and narration included — which the bridge's census
    // can vouch for exactly however much of the run the page shipped.
    expect(group.querySelector(".thread-activity-count").textContent).toBe("5");
    // The last call, not the last row: the line says what the agent reached
    // for, and a live run's line is a ticker over its calls.
    expect(group.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
  });

  // No label. "Background task — started — run the full suite" spends the
  // width of the line on the half the reader can already see is activity.
  it("says no label on the collapsed line", () => {
    document.body.innerHTML = activity();

    const head = groups()[0].querySelector(".thread-activity-head");
    expect(head.querySelector(".thread-activity-what")).toBe(null);
    expect(head.textContent).not.toContain("Background task");
    expect(head.textContent).not.toContain("Agent");
  });

  // What ends a run: anybody speaking. Activity is the gap between two things
  // that were said, and the point of collapsing it is that the saying stays
  // findable.
  it("starts a new run after a message", () => {
    document.body.innerHTML = openThreadHtml({
      items: [
        event({ event: "reasoning", summary: "Checking the lock order." }),
        event({ event: "tool_use", summary: "Read bridge/src/app.rs" }),
        message("The lock has to move."),
        event({ event: "tool_use", summary: "Edit bridge/src/app.rs" }),
      ],
    });

    expect(timeline().map((row) => row.className.split(" ")[0])).toEqual([
      "thread-activity-group",
      "thread-message",
      "thread-activity-group",
    ]);
    expect(groups().map((group) => group.querySelector(".thread-activity-count").textContent)).toEqual(["2", "1"]);
    expect(groups()[0].querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
    expect(document.querySelector(".thread-message").textContent).toContain("The lock has to move.");
  });

  // A lifecycle row is not activity — it is something that HAPPENED, and it
  // carries a tone and a summons. It ends the run like a message does.
  it("starts a new run after a lifecycle event", () => {
    document.body.innerHTML = openThreadHtml({
      items: [
        event({ event: "narration", summary: "Running the suite." }),
        event({ event: "done", summary: "Implemented the requested change." }),
        event({ event: "reasoning", summary: "Reading the review." }),
      ],
    });

    expect(groups()).toHaveLength(2);
    expect(timeline()[1].classList.contains("thread-event")).toBe(true);
    expect(timeline()[1].textContent).toContain("Agent reported done");
  });

  // An event kind this client has never heard of is not swept into the run:
  // it renders as the plain row it always did, and it ends the run, exactly as
  // the timeline treated it before runs existed.
  it("leaves an unknown kind out of the run, and ends the run with it", () => {
    document.body.innerHTML = openThreadHtml({
      items: [
        event({ event: "tool_use", summary: "Read bridge/src/app.rs" }),
        event({ event: "some_new_kind", summary: "Something happened." }),
        event({ event: "tool_result", summary: "17 matches" }),
      ],
    });

    expect(groups()).toHaveLength(2);
    expect(timeline()[1].classList.contains("thread-activity-group")).toBe(false);
    expect(timeline()[1].textContent).toContain("some new kind");
    // The legacy standalone answer row is activity, and rides the run.
    expect(groups()[1].querySelector(".thread-activity-preview").textContent).toBe("17 matches");
  });

  // One item is still a run. The line reads the same, so a run that grows
  // under a reader watching it never changes shape — and the row underneath is
  // one press away either way.
  it("collapses a run of one", () => {
    document.body.innerHTML = openThreadHtml({
      items: [event({ event: "tool_use", summary: "Read bridge/src/app.rs\n\nlines 1-40" })],
    });

    const group = groups()[0];
    expect(group.querySelector(".thread-activity-count").textContent).toBe("1");
    expect(group.querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
  });

  // How the newest call ended rides the line: the reader watching a run go by
  // sees the last thing tried and whether it worked, which is the whole of what
  // a ticker is for.
  it("carries the latest call's mark on the line", () => {
    document.body.innerHTML = openThreadHtml({
      items: [
        event({ event: "reasoning", summary: "The suite should be green." }),
        event({ event: "tool_use", summary: "Bash npm test\n→ 3 tests failed", outcome: "error" }),
      ],
    });

    const mark = groups()[0].querySelector(".thread-activity-head .thread-activity-outcome");
    expect(mark.dataset.outcome).toBe("error");
    expect(mark.classList.contains("blocked")).toBe(true);
  });

  // The count on the conversation's own title counts what was said and done,
  // not how many runs it fell into.
  it("still counts every item in the conversation's title", () => {
    document.body.innerHTML = activity();

    expect(document.querySelector(".thread-title-text").textContent).toContain("5");
  });
});

// A run the pane has open. The press that opens one is the pane's to hear —
// what it draws once it is open is here.
describe("an open run", () => {
  it("shows every row in it, exactly as activity renders on its own", () => {
    document.body.innerHTML = openActivity();
    const group = document.querySelector(".thread-activity-group");

    expect(group.open).toBe(true);
    const list = group.querySelector(".thread-activity-group-list");
    const rows = [...list.querySelectorAll(".thread-activity")];
    expect(rows).toHaveLength(5);
    expect(rows.map((row) => row.querySelector(".thread-event-icon").getAttribute("aria-label"))).toEqual([
      "Agent thought",
      "Agent called a tool",
      "Tool answered",
      "Agent narrated",
      "Background task",
    ]);
    expect(rows[1].querySelector(".thread-activity-preview").textContent).toBe("Read bridge/src/app.rs");
  });

  it("keeps every child row's one-line preview visible when the run and row are open", () => {
    document.body.innerHTML = openActivity();
    const group = document.querySelector(".thread-activity-group");
    const row = group.querySelector(".thread-activity");
    row.open = true;

    expect(row.querySelector(".thread-activity-preview").textContent).toBe(ACTIVITY_ITEMS[0].data.summary);
    expect(ACTIVITY_STYLES).not.toMatch(/\.thread-activity\[open\][^{]*\.thread-activity-preview/);
    expect(ACTIVITY_STYLES).not.toMatch(/\.thread-activity-group\[open\]\s+\.thread-activity-preview/);
  });

  it("renders the full available expanded text through 5000 characters", () => {
    const fullEntry = `Inspect output\n\n${"x".repeat(4984)}`;
    document.body.innerHTML = openThreadHtml({
      items: [{ type: "event", data: { event: "reasoning", summary: fullEntry } }],
    });

    const detail = document.querySelector(".thread-event-detail").textContent;
    expect(detail).toContain("Inspect output");
    expect(detail).toContain("x".repeat(4984));
    expect(detail).not.toContain("…");
  });

  // The head is a `<summary>`, so the press that shuts a run is the browser's
  // own — the pane hears it and stops drawing the children, but the box the
  // reader pressed shuts under their finger either way.
  it("shuts again on the line that opened it", () => {
    document.body.innerHTML = openActivity();
    const group = document.querySelector(".thread-activity-group");

    group.querySelector(".thread-activity-head").click();

    expect(group.open).toBe(false);
  });

  // A shut run holds nothing: the whole point of the fold is that a thousand
  // calls nobody asked to see are a thousand rows the document never holds.
  it("draws nothing under a run the reader has not opened", () => {
    document.body.innerHTML = activity();

    expect(document.querySelector(".thread-activity-group-list")).toBe(null);
    expect(document.querySelectorAll(".thread-activity")).toHaveLength(0);
  });
});

// The run a reader opened is the run they are reading, and the poll repaints
// the conversation under them every 1.6 seconds. It has to stay open — and it
// has to keep being the same element, or the browser drops the scroll position
// inside it.
describe("a run the reader opened", () => {
  const toolCall = (sequence) => ({
    type: "event",
    data: { sequence, event: "tool_use", summary: `Read src/a${sequence}.js` },
  });
  const run = (calls) => openThreadHtml({ items: calls.map((sequence) => toolCall(sequence)) });

  it("stays open across the repaint, and takes in what arrived", () => {
    document.body.innerHTML = run([1, 2, 3]);
    const live = document.querySelector(".thread-items");
    const group = live.querySelector(".thread-activity-group");
    group.open = true;
    const runKey = group.dataset.activityRun;

    const rendered = document.createElement("div");
    rendered.innerHTML = run([1, 2, 3, 4]);
    patchElement(live, rendered.querySelector(".thread-items"));

    const patched = live.querySelector(".thread-activity-group");
    // The same element, so what the reader had scrolled to inside it is still
    // where they left it.
    expect(patched).toBe(group);
    expect(patched.open).toBe(true);
    // Keyed by the run's FIRST item, so a run that grows keeps its identity.
    expect(patched.dataset.activityRun).toBe(runKey);
    expect(patched.querySelector(".thread-activity-count").textContent).toBe("4");
    expect(patched.querySelector(".thread-activity-preview").textContent).toBe("Read src/a4.js");
    expect(patched.querySelectorAll(".thread-activity-group-list .thread-activity")).toHaveLength(4);
  });
});
