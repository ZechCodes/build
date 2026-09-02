// @vitest-environment jsdom
// A subagent's rows fold under the tool call that spawned them.
//
// The agent that spawned them is one row in this conversation — the Agent call
// — and everything it said belongs inside that row rather than beside it. The
// window the reader is standing in decides: a row whose parent has not been
// fetched yet is a row of its own, and joins its parent the moment the parent
// arrives.

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

const laterCall = {
  type: "event",
  data: {
    event: "tool_use",
    sequence: 12,
    summary: "Read bridge/src/app.rs",
    created_at: "2026-09-01T12:00:02Z",
  },
};

const paint = (items) => {
  document.body.innerHTML = threadHtml({ items });
  return document.querySelector(".thread-items");
};

const rowOfSequence = (sequence) => document.querySelector(`[data-sequence="${sequence}"]`);

describe("rows that fold under the call that spawned them", () => {
  it("draws a parented row inside its parent's fold and never as a row of its own", () => {
    paint([spawningCall, subagentThought, laterCall]);

    const runs = [...document.querySelectorAll(".thread-activity-group")];
    expect(runs).toHaveLength(1);
    // Two rows the reader can see, so the ticker counts two.
    expect(runs[0].querySelector(".thread-activity-group-list").children).toHaveLength(2);
    expect(runs[0].querySelector(".thread-activity-count").textContent).toBe("2");

    const parent = rowOfSequence(SPAWNING_CALL_SEQUENCE);
    expect(parent.tagName).toBe("DETAILS");
    const children = parent.querySelector(".thread-activity-children");
    expect(children.textContent).toContain("The lock has to move");
    expect(document.querySelectorAll(".thread-activity-children")).toHaveLength(1);
  });

  it("draws a row whose parent is above the window flat, exactly once, until the parent arrives", () => {
    paint([subagentThought, laterCall]);

    expect(document.querySelectorAll(".thread-activity-children")).toHaveLength(0);
    expect(document.querySelector(".thread-activity-group-list").children).toHaveLength(2);
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

  it("does nothing for a sequence no row carries", () => {
    const scroller = paint([spawningCall, laterCall]);

    expect(revealThreadSequence(scroller, 4242)).toBe(false);
    expect(document.querySelector(".thread-activity-group").open).toBe(false);
  });
});
