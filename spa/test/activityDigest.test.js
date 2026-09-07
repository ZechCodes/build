// The count and the last call on a folded run of activity.
//
// A folded run's tool-call count is a fact about the WHOLE run, and the whole
// run is not what the page ships: the bridge caps how much of one run travels
// and sends a digest for the rest. So the row is drawn from the digest, topped
// up with what arrived after it, and never from how many rows happen to be in
// hand.

import { describe, expect, it } from "vitest";
import {
  activityRunSummary,
  digestCovering,
  firstLine,
  mergeActivityDigests,
  runDigestToFetch,
} from "../src/core/activityDigest.js";

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

const call = (sequence, meat, extra = {}) => ({
  sequence,
  meat,
  outcome: extra.outcome,
  createdAt: extra.createdAt ?? null,
});

const entry = (sequence, kind, meat, extra = {}) => ({
  html: "",
  key: sequence,
  activity: {
    icon: "▸",
    meat,
    sequence,
    outcome: undefined,
    createdAt: null,
    toolCalls: kind === "tool_use" ? [call(sequence, meat, extra)] : [],
    ...extra,
  },
});

/// A tool call that spawned a subagent: the calls the subagent made fold under
/// it, so the run holds one row standing for several calls.
const spawningEntry = (sequence, meat, nested) => ({
  html: "",
  key: sequence,
  activity: {
    icon: "▸",
    meat,
    sequence,
    outcome: undefined,
    createdAt: null,
    toolCalls: [call(sequence, meat), ...nested],
  },
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

  // A subagent's calls fold under the call that spawned them, so they never
  // become rows of their own — and a count of rows would say one where the
  // bridge counted four.
  it("counts the calls folded under a row, not the rows", () => {
    const run = [
      spawningEntry(2, "Task(review the parser)", [
        call(3, "Read spa/src/core/thread.js"),
        call(4, "Grep patchList"),
        call(5, "Bash(npm test)"),
      ]),
    ];

    expect(activityRunSummary([], run).count).toBe(4);
    expect(activityRunSummary([], run).meat).toBe("Bash(npm test)");
  });

  // Two subagents working at once interleave their calls in the run: the
  // subtree of the one that started first holds the newest call, and it sits
  // in the middle of the list rather than at the end of it.
  it("takes the newest call by sequence, not the last one in the list", () => {
    const run = [
      spawningEntry(2, "Task(review the parser)", [
        call(5, "Read spa/src/core/thread.js"),
        call(7, "Grep patchList"),
        call(9, "Bash(npm test)", { outcome: "ok", createdAt: "2026-09-06T18:09:00.000Z" }),
      ]),
      spawningEntry(3, "Task(review the lexer)", [
        call(4, "Read bridge/src/app.rs"),
        call(6, "Grep tool_call"),
        call(8, "Bash(cargo test)", { outcome: "error", createdAt: "2026-09-06T18:08:00.000Z" }),
      ]),
    ];

    const summary = activityRunSummary([], run);

    expect(summary.count).toBe(8);
    expect(summary.meat).toBe("Bash(npm test)");
    expect(summary.outcome).toBe("ok");
    expect(summary.createdAt).toBe("2026-09-06T18:09:00.000Z");
  });

  it("tops a digest up with the folded calls that arrived after it", () => {
    const run = [
      spawningEntry(2, "Task(review the parser)", [
        call(3, "Read spa/src/core/thread.js"),
        call(4, "Grep patchList"),
        call(5, "Bash(npm test)"),
      ]),
    ];

    expect(activityRunSummary([digest(2, 2, 1, lastCall(2, "Task(review the parser)"))], run).count).toBe(4);
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

  // The head's line, mark, time and glyph are one reading of one thing. A run
  // that ends in a thought still says what the agent last DID, so the glyph
  // beside that line is a tool call's and not the thought's.
  it("says a tool call's glyph whenever the line came from a call", () => {
    const heldCall = [
      entry(1530, "tool_use", "Bash(cargo test)", { outcome: "ok", createdAt: "2026-09-06T18:03:11.412Z" }),
      entry(1531, "reasoning", "The suite is green.", { icon: "\u25cc" }),
    ];

    expect(activityRunSummary([], heldCall).icon).toBe("\u25b8");

    const digestedCall = [entry(1600, "reasoning", "Reading the review.", { icon: "\u25cc" })];

    expect(
      activityRunSummary([digest(412, 1600, 1000, lastCall(1530, "Bash(cargo test)"))], digestedCall).icon,
    ).toBe("\u25b8");
  });

  it("says the latest row's own glyph when the line fell back to that row", () => {
    const noCall = [
      entry(1600, "tool_use", "Read a.js", { icon: "\u25b8" }),
      entry(1601, "narration", "Running the suite.", { icon: "\u25e6", toolCalls: [] }),
    ];

    expect(activityRunSummary([digest(1600, 1601, 0, null)], noCall).icon).toBe("\u25e6");

    const wordlessDigest = [entry(1600, "reasoning", "Reading the review.", { icon: "\u25cc" })];

    expect(activityRunSummary([digest(412, 1600, 4, lastCall(1530, null, null, null))], wordlessDigest).icon).toBe(
      "\u25cc",
    );
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

  it("counts a run rendered without sequences by the calls it holds", () => {
    const run = [
      { html: "", key: "at-0", activity: { icon: "▸", meat: "Read a.js", toolCalls: [{ meat: "Read a.js" }] } },
      { html: "", key: "at-1", activity: { icon: "◌", meat: "Thinking.", toolCalls: [] } },
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

// A run's key is the oldest sequence the WINDOW holds of it, which is not
// where the run started when the page cut it. So the pane finds a run's digest
// by what it covers, never by its first sequence.
describe("the digest over a sequence", () => {
  it("answers the digest whose run covers it, at either end of the span", () => {
    const digests = [digest(1, 40, 12), digest(120, 870, 300)];

    expect(digestCovering(digests, 400)).toBe(digests[1]);
    expect(digestCovering(digests, 120)).toBe(digests[1]);
    expect(digestCovering(digests, 870)).toBe(digests[1]);
    expect(digestCovering(digests, 40)).toBe(digests[0]);
  });

  it("answers nothing for a sequence no digest reaches", () => {
    expect(digestCovering([digest(1, 40, 12)], 900)).toBe(null);
    expect(digestCovering([], 900)).toBe(null);
    expect(digestCovering(null, 900)).toBe(null);
  });
});

// Which pressed runs are a fetch, and which are already in hand.
describe("the run a press has to fetch", () => {
  const digests = [digest(10, 51, 40), digest(60, 90, 8)];

  it("asks over the digest of a run the page cut", () => {
    expect(runDigestToFetch(digests, 50, 200)).toEqual(digests[0]);
  });

  it("asks for nothing when the window holds the run whole", () => {
    expect(runDigestToFetch([digest(60, 90, 8)], 60, 200)).toBe(null);
  });

  it("asks for nothing when the run reaches the end of the conversation", () => {
    expect(runDigestToFetch(digests, 50, 51)).toBe(null);
  });

  it("asks for nothing for a sequence no digest covers", () => {
    expect(runDigestToFetch(digests, 55, 200)).toBe(null);
  });
});
