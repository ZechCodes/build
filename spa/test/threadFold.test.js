// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { revealThreadSequence, threadHtml } from "../src/core/thread.js";

const SPAWNING_CALL_SEQUENCE = 10;

const spawningCall = {
  type: "event",
  data: {
    event: "tool_use",
    sequence: SPAWNING_CALL_SEQUENCE,
    summary: "Agent review the parser",
    created_at: "2026-09-01T12:00:00Z",
  },
};

const subagentThought = {
  type: "event",
  data: {
    event: "reasoning",
    sequence: 11,
    parent_sequence: SPAWNING_CALL_SEQUENCE,
    summary: "The lock has to move before the parser can be re-entrant.",
    created_at: "2026-09-01T12:00:01Z",
  },
};

const subagentCall = {
  type: "event",
  data: {
    event: "tool_use",
    sequence: 11,
    parent_sequence: SPAWNING_CALL_SEQUENCE,
    summary: "Agent read the parser",
    created_at: "2026-09-01T12:00:01Z",
  },
};

const grandchildCall = {
  type: "event",
  data: {
    event: "tool_use",
    sequence: 20,
    parent_sequence: 11,
    summary: "Read spa/src/core/patchList.js",
    created_at: "2026-09-01T12:00:02Z",
  },
};

const laterCall = {
  type: "event",
  data: {
    event: "tool_use",
    sequence: 12,
    summary: "Read bridge/src/app.rs",
    created_at: "2026-09-01T12:00:02Z",
  },
};

// What folds under which row is what these tests are about, and a run draws
// what it stands for only when the reader has it open (core/thread.js
// `activityRunHtml`). So every run is open here.
const EVERY_RUN_OPEN = { has: () => true };

const paint = (items) => {
  document.body.innerHTML = threadHtml({ items }, { openRuns: EVERY_RUN_OPEN });
  return document.querySelector(".thread-items");
};

const rowOfSequence = (sequence) => document.querySelector(`[data-sequence="${sequence}"]`);

describe("rows that fold under the call that spawned them", () => {
  it("draws a parented row inside its parent's fold and never as a row of its own", () => {
    paint([spawningCall, subagentThought, laterCall]);

    const runs = [...document.querySelectorAll(".thread-activity-group")];
    expect(runs).toHaveLength(1);
    const rowsTheReaderCanSee = 2;
    expect(runs[0].querySelector(".thread-activity-group-list").children).toHaveLength(rowsTheReaderCanSee);
    // Two rows to see, three in the run: the folded thought counts too.
    expect(runs[0].querySelector(".thread-activity-count").textContent).toBe("3");

    const parent = rowOfSequence(SPAWNING_CALL_SEQUENCE);
    expect(parent.tagName).toBe("DETAILS");
    const children = parent.querySelector(".thread-activity-children");
    expect(children.textContent).toContain("The lock has to move");
    expect(document.querySelectorAll(".thread-activity-children")).toHaveLength(1);
  });

  it("folds a spawned agent's own spawn under it, at every depth", () => {
    paint([spawningCall, subagentCall, grandchildCall, laterCall]);

    for (const sequence of [SPAWNING_CALL_SEQUENCE, 11, 20, 12]) {
      expect(document.querySelectorAll(`[data-sequence="${sequence}"]`)).toHaveLength(1);
    }
    expect(rowOfSequence(11).closest(".thread-activity-children").parentElement).toBe(
      rowOfSequence(SPAWNING_CALL_SEQUENCE),
    );
    expect(rowOfSequence(20).closest(".thread-activity-children").parentElement).toBe(rowOfSequence(11));

    const rowsTheReaderCanSee = 2;
    expect(document.querySelector(".thread-activity-group-list").children).toHaveLength(rowsTheReaderCanSee);
    // Two rows to see, four in the run: the spawning call stands for
    // everything the subagent it started did, at every depth.
    expect(document.querySelector(".thread-activity-count").textContent).toBe("4");
  });

  it("draws a row once and stops when two rows name each other as parent", () => {
    const first = { type: "event", data: { event: "tool_use", sequence: 30, parent_sequence: 31, summary: "first" } };
    const second = { type: "event", data: { event: "tool_use", sequence: 31, parent_sequence: 30, summary: "second" } };

    paint([first, second, laterCall]);

    for (const sequence of [30, 31]) {
      expect(document.querySelectorAll(`[data-sequence="${sequence}"]`).length).toBeLessThanOrEqual(1);
    }
    expect(rowOfSequence(12)).not.toBe(null);
  });

  it("draws a row whose parent is above the window flat, exactly once, until the parent arrives", () => {
    paint([subagentThought, laterCall]);

    expect(document.querySelectorAll(".thread-activity-children")).toHaveLength(0);
    expect(document.querySelector(".thread-activity-group-list").children).toHaveLength(2);
    // Both rows count, the thought as much as the call.
    expect(document.querySelector(".thread-activity-count").textContent).toBe("2");
    expect(document.querySelectorAll(`[data-sequence="${subagentThought.data.sequence}"]`)).toHaveLength(1);

    paint([spawningCall, subagentThought, laterCall]);

    expect(document.querySelectorAll(".thread-activity-children")).toHaveLength(1);
    expect(document.querySelector(".thread-activity-group-list").children).toHaveLength(2);
  });

  it("reveals the row a sequence names, opening every fold over it", () => {
    const scroller = paint([spawningCall, subagentThought, laterCall]);

    expect(revealThreadSequence(scroller, SPAWNING_CALL_SEQUENCE)).toBe(true);
    expect(rowOfSequence(SPAWNING_CALL_SEQUENCE).open).toBe(true);
    expect(document.querySelector(".thread-activity-group").open).toBe(true);
  });

  it("does nothing for a sequence no row carries, or for one that is not a number", () => {
    const scroller = paint([spawningCall, laterCall]);

    expect(revealThreadSequence(scroller, 4242)).toBe(false);
    expect(revealThreadSequence(scroller, '"] , [data-sequence')).toBe(false);
  });
});
