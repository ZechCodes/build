// @vitest-environment jsdom
// The timeline as keyed rows.
//
// One entry per top-level row — a message under its sequence, a folded run
// under the sequence it starts at, a lifecycle event under its own — so the
// keyed reconciler leaves a row nobody changed exactly where it is. A folded
// run is a HEAD: what it stands for is drawn only when it is open, which is
// what keeps a thousand-call run out of the document until somebody asks.

import { describe, expect, it } from "vitest";
import { activityRunKeyAt, pressedActivityRunKey, timelineEntries } from "../src/core/thread.js";

const toolCall = (sequence, summary, extra = {}) => ({
  type: "event",
  data: { sequence, event: "tool_use", summary, created_at: "2026-09-06T18:03:11.412Z", ...extra },
});

const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

const lifecycle = (sequence) => ({
  type: "event",
  data: { sequence, event: "done", summary: "Reported done", created_at: "2026-09-06T18:04:00.000Z" },
});

const build = (items, options = {}) => timelineEntries(items, "Claude Code", "th-1", options.digests || [], options);

const keysOf = (built) => built.entries.map((entry) => entry.key);

const drawn = (built) => {
  document.body.innerHTML = `<div class="thread-items">${built.entries.map((entry) => entry.html).join("")}</div>`;
  return document.querySelector(".thread-items");
};

describe("the timeline's keyed rows", () => {
  it("keys a message by its sequence, a run by its first, and an event by its own", () => {
    const built = build([message(1, "start"), toolCall(2, "Read a.js"), toolCall(3, "Read b.js"), lifecycle(4)]);

    expect(keysOf(built)).toEqual(["1", "2", "4"]);
    expect(built.itemCount).toBe(4);
  });

  it("names a run's key on the row itself, as the fold has always been named", () => {
    const built = build([toolCall(7, "Read a.js"), toolCall(8, "Read b.js")]);
    const run = drawn(built).querySelector(".thread-activity-group");

    expect(run.dataset.activityRun).toBe("7");
    expect(run.dataset.activityThrough).toBe("8");
  });

  it("draws a collapsed run as a head with nothing under it", () => {
    const built = build([toolCall(7, "Read a.js"), toolCall(8, "Read b.js")]);
    const timeline = drawn(built);

    expect(timeline.querySelector(".thread-activity-group-list")).toBe(null);
    expect(timeline.textContent).not.toContain("Read a.js");
    expect(timeline.querySelector(".thread-activity-count").textContent).toBe("2");
    expect(timeline.querySelector(".thread-activity-group").open).toBe(false);
  });

  it("draws an open run's children from the window, live on every delta", () => {
    const held = [toolCall(7, "Read a.js"), toolCall(8, "Read b.js")];
    const open = { openRuns: new Set(["7"]) };
    const timeline = drawn(build(held, open));

    expect(timeline.querySelector(".thread-activity-group").open).toBe(true);
    expect(timeline.querySelectorAll(".thread-activity-group-list > .thread-activity")).toHaveLength(2);

    const grown = drawn(build([...held, toolCall(9, "Read c.js")], open));

    expect(grown.querySelectorAll(".thread-activity-group-list > .thread-activity")).toHaveLength(3);
    expect(grown.textContent).toContain("Read c.js");
  });

  it("draws an open run's fetched items, with anything newer than them beside", () => {
    const built = build([toolCall(20, "Read c.js"), toolCall(21, "Read d.js")], {
      openRuns: new Set(["20"]),
      runItemsOf: (key) => (key === "20" ? [toolCall(18, "Read a.js"), toolCall(19, "Read b.js"), toolCall(20, "Read c.js")] : undefined),
    });
    const rows = [...drawn(built).querySelectorAll(".thread-activity-group-list > .thread-activity")];

    expect(rows.map((row) => row.dataset.sequence)).toEqual(["18", "19", "20", "21"]);
  });

  it("falls back to the window when the fetch has not landed", () => {
    const built = build([toolCall(20, "Read c.js")], { openRuns: new Set(["20"]), runItemsOf: () => undefined });

    expect(drawn(built).querySelectorAll(".thread-activity-group-list > .thread-activity")).toHaveLength(1);
  });

  it("says the same bytes for a row nothing moved under", () => {
    const items = [message(1, "start"), toolCall(2, "Read a.js")];

    expect(build(items).entries).toEqual(build(items).entries);
  });

  it("draws the run open without asking for children a second time", () => {
    const items = [toolCall(2, "Read a.js")];

    expect(build(items, { openRuns: new Set(["2"]) }).entries[0].html).not.toBe(build(items).entries[0].html);
  });
});

// A shut run holds no rows, so the only way back to a call folded inside one is
// the span the head carries. Both of these are what the pane presses on.
describe("finding a run in the document", () => {
  const spawningCall = toolCall(7, "Task(review the parser)");
  const subagentCall = toolCall(9, "Read a.js", { parent_sequence: 7 });

  it("answers the run a press landed on, and nothing for a press beside one", () => {
    const timeline = drawn(build([spawningCall, subagentCall]));

    expect(pressedActivityRunKey(timeline.querySelector(".thread-activity-preview"))).toBe("7");
    expect(pressedActivityRunKey(timeline)).toBe(null);
    expect(pressedActivityRunKey(null)).toBe(null);
  });

  it("answers the run a call is folded into, including one drawn under another call", () => {
    const timeline = drawn(build([spawningCall, subagentCall]));

    expect(activityRunKeyAt(timeline, 9)).toBe("7");
    expect(activityRunKeyAt(timeline, 4242)).toBe(null);
    expect(activityRunKeyAt(timeline, "not a sequence")).toBe(null);
  });
});
