// The count and the last call on a folded run of activity.
//
// A folded run's tool-call count is a fact about the WHOLE run, and the whole
// run is not what the page ships: the bridge caps how much of one run travels
// and sends a digest for the rest. So the row is drawn from the digest, topped
// up with what arrived after it, and never from how many rows happen to be in
// hand.

import { describe, expect, it } from "vitest";
import { activityRunSummary, firstLine, mergeActivityDigests } from "../src/core/activityDigest.js";

const digest = (from, through, toolCalls, lastToolCall = null) => ({
  from_sequence: from,
  through_sequence: through,
  tool_calls: toolCalls,
  last_tool_call: lastToolCall,
});

const lastCall = (sequence, summary, outcome = "ok", createdAt = "2026-09-06T18:03:11.412Z") => ({
  sequence,
  created_at: createdAt,
  summary,
  outcome,
});

const entry = (sequence, kind, meat, extra = {}) => ({
  html: "",
  key: sequence,
  activity: { icon: "▸", kind, meat, sequence, outcome: undefined, createdAt: null, ...extra },
});

describe("holding the digests a page carried", () => {
  it("leaves what is held alone when the payload carries none", () => {
    const held = [digest(1, 9, 3)];

    expect(mergeActivityDigests(held, { items: [] })).toBe(held);
    expect(mergeActivityDigests(held, null)).toBe(held);
    expect(mergeActivityDigests(held, { activity_digests: [] })).toBe(held);
  });

  it("replaces the digest for a run it already held, and keeps the rest", () => {
    const held = [digest(1, 9, 3), digest(20, 40, 7)];

    const merged = mergeActivityDigests(held, { activity_digests: [digest(20, 55, 12)] });

    expect(merged.map((entryDigest) => [entryDigest.from_sequence, entryDigest.tool_calls])).toEqual([
      [1, 3],
      [20, 12],
    ]);
    expect(held[1].tool_calls).toBe(7);
  });

  it("takes in the runs an older page reaches back to, oldest first", () => {
    const held = [digest(20, 40, 7)];

    const merged = mergeActivityDigests(held, { activity_digests: [digest(1, 9, 3), digest(11, 15, 1)] });

    expect(merged.map((entryDigest) => entryDigest.from_sequence)).toEqual([1, 11, 20]);
  });

  it("opens on the digests of a first page when nothing is held", () => {
    expect(mergeActivityDigests([], { activity_digests: [digest(4, 8, 2)] })).toHaveLength(1);
  });
});

describe("what a folded run says", () => {
  it("says the digest's count, not how many rows are in hand", () => {
    const run = [entry(1530, "tool_use", "Bash(cargo test)"), entry(1531, "reasoning", "The suite is green.")];

    const summary = activityRunSummary([digest(412, 1531, 1000, lastCall(1530, "Bash(cargo test)"))], run);

    expect(summary.count).toBe(1000);
  });

  it("adds the calls that arrived after the digest was cut", () => {
    const run = [
      entry(1530, "tool_use", "Bash(cargo test)"),
      entry(1532, "tool_use", "Edit bridge/src/app.rs"),
      entry(1533, "tool_use", "Read spa/src/core/thread.js"),
    ];

    const summary = activityRunSummary([digest(412, 1531, 1000, lastCall(1530, "Bash(cargo test)"))], run);

    expect(summary.count).toBe(1002);
    expect(summary.meat).toBe("Read spa/src/core/thread.js");
  });

  // Startup rows and hidden messages are filtered before folding, so one run
  // on this side can be the two the bridge cut either side of what it dropped.
  it("sums every digest the run reaches over", () => {
    const run = [entry(10, "tool_use", "a"), entry(60, "tool_use", "b")];

    const summary = activityRunSummary([digest(5, 30, 40), digest(31, 70, 60), digest(200, 300, 9)], run);

    expect(summary.count).toBe(100);
  });

  it("counts the rows in hand when no digest reaches the run", () => {
    const run = [
      entry(10, "tool_use", "Read a.js"),
      entry(11, "reasoning", "Thinking."),
      entry(12, "tool_use", "Read b.js"),
    ];

    expect(activityRunSummary([digest(500, 600, 40)], run).count).toBe(2);
    expect(activityRunSummary([], run).count).toBe(2);
  });

  it("takes the line, the mark and the time from the newest call in hand", () => {
    const run = [
      entry(1530, "tool_use", "Bash(cargo test)", { outcome: "ok", createdAt: "2026-09-06T18:03:11.412Z" }),
      entry(1533, "tool_use", "Edit bridge/src/app.rs", {
        outcome: "error",
        createdAt: "2026-09-06T18:05:00.000Z",
      }),
      entry(1534, "reasoning", "The edit failed."),
    ];

    const summary = activityRunSummary([digest(412, 1531, 1000, lastCall(1530, "Bash(cargo test)"))], run);

    expect(summary.meat).toBe("Edit bridge/src/app.rs");
    expect(summary.outcome).toBe("error");
    expect(summary.createdAt).toBe("2026-09-06T18:05:00.000Z");
  });

  it("falls back to the digest's last call when the run holds none", () => {
    const run = [entry(1600, "reasoning", "Reading the review.")];

    const summary = activityRunSummary(
      [digest(412, 1600, 1000, lastCall(1530, "Bash(cargo test)\n→ 412 passed", "error", "2026-09-06T18:03:11.412Z"))],
      run,
    );

    expect(summary.count).toBe(1000);
    expect(summary.meat).toBe("Bash(cargo test)");
    expect(summary.outcome).toBe("error");
    expect(summary.createdAt).toBe("2026-09-06T18:03:11.412Z");
  });

  it("shows the latest row when the digest's last call carries no words", () => {
    const run = [entry(1600, "reasoning", "Reading the review.", { createdAt: "2026-09-06T19:00:00.000Z" })];

    const summary = activityRunSummary([digest(412, 1600, 4, lastCall(1530, null, null, null))], run);

    expect(summary.count).toBe(4);
    expect(summary.meat).toBe("Reading the review.");
    expect(summary.createdAt).toBe("2026-09-06T19:00:00.000Z");
  });

  it("keeps the old look for a run that called no tool at all", () => {
    const run = [
      entry(1600, "reasoning", "Reading the review."),
      entry(1601, "narration", "Running the suite.", { createdAt: "2026-09-06T19:00:00.000Z" }),
    ];

    const summary = activityRunSummary([digest(1600, 1601, 0, null)], run);

    expect(summary.count).toBe(2);
    expect(summary.meat).toBe("Running the suite.");
    expect(summary.createdAt).toBe("2026-09-06T19:00:00.000Z");
  });

  it("counts a run rendered without sequences by the rows it holds", () => {
    const run = [
      { html: "", key: "at-0", activity: { icon: "▸", kind: "tool_use", meat: "Read a.js" } },
      { html: "", key: "at-1", activity: { icon: "◌", kind: "reasoning", meat: "Thinking." } },
    ];

    expect(activityRunSummary([digest(1, 9, 40)], run).count).toBe(1);
  });
});

describe("the line of a summary", () => {
  it("is the first line with anything on it", () => {
    expect(firstLine("\n\nBash(cargo test)\n→ 412 passed")).toBe("Bash(cargo test)");
    expect(firstLine("")).toBe("");
  });
});
