// @vitest-environment jsdom
// The number on a folded run.
//
// A run of a thousand tool calls ships as a hundred: the bridge caps how much
// of one run travels and sends the run's total beside it. So the row counts
// TOOL CALLS, from the digest, and shows the last call the agent made — never
// how many rows happened to arrive.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

const toolCall = (sequence, summary, extra = {}) => ({
  type: "event",
  data: { sequence, event: "tool_use", summary, created_at: "2026-09-06T18:03:11.412Z", ...extra },
});

const reasoning = (sequence, summary) => ({
  type: "event",
  data: { sequence, event: "reasoning", summary, created_at: "2026-09-06T18:03:11.412Z" },
});

const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

const paint = (thread) => {
  document.body.innerHTML = threadHtml(thread);
  return document.querySelector(".thread-activity-group");
};

const countOn = (group) => group.querySelector(".thread-activity-count").textContent;
const previewOn = (group) => group.querySelector(".thread-activity-preview").textContent;

describe("a folded run drawn from its digest", () => {
  const digest = (from, through, toolCalls, lastToolCall = null) => ({
    from_sequence: from,
    through_sequence: through,
    tool_calls: toolCalls,
    last_tool_call: lastToolCall,
  });

  it("says what the bridge counted, not how many rows arrived", () => {
    const group = paint({
      items: [toolCall(1529, "Read spa/src/core/thread.js"), toolCall(1530, "Bash(cargo test)", { outcome: "ok" })],
      activityDigests: [
        digest(412, 1531, 1000, {
          sequence: 1530,
          created_at: "2026-09-06T18:03:11.412Z",
          summary: "Bash(cargo test)",
          outcome: "ok",
        }),
      ],
    });

    expect(countOn(group)).toBe("1000");
    expect(previewOn(group)).toBe("Bash(cargo test)");
    expect(group.querySelector(".thread-activity-outcome").dataset.outcome).toBe("ok");
    expect(group.querySelectorAll(".thread-activity-group-list .thread-activity")).toHaveLength(2);
  });

  it("adds the calls that arrived after the page was cut", () => {
    const group = paint({
      items: [toolCall(1530, "Bash(cargo test)"), toolCall(1532, "Edit bridge/src/app.rs")],
      activityDigests: [digest(412, 1531, 1000, { sequence: 1530, summary: "Bash(cargo test)", outcome: "ok" })],
    });

    expect(countOn(group)).toBe("1001");
    expect(previewOn(group)).toBe("Edit bridge/src/app.rs");
  });

  it("shows the last call the digest recorded when the run holds none", () => {
    const group = paint({
      items: [reasoning(1600, "Reading the review.")],
      activityDigests: [
        digest(412, 1600, 1000, {
          sequence: 1530,
          created_at: "2026-09-06T18:03:11.412Z",
          summary: "Bash(cargo test)\n→ 412 passed",
          outcome: "error",
        }),
      ],
    });

    expect(countOn(group)).toBe("1000");
    expect(previewOn(group)).toBe("Bash(cargo test)");
    expect(group.querySelector(".thread-activity-outcome").dataset.outcome).toBe("error");
  });

  it("gives each run between messages its own digest", () => {
    document.body.innerHTML = threadHtml({
      items: [
        toolCall(10, "Read a.js"),
        message(11, "Reading the parser."),
        toolCall(12, "Read b.js"),
      ],
      activityDigests: [digest(1, 10, 40), digest(12, 12, 300)],
    });

    expect(
      [...document.querySelectorAll(".thread-activity-group")].map((group) => countOn(group)),
    ).toEqual(["40", "300"]);
  });

  // A conversation shipped whole, and every run written since the page was
  // cut: nothing has a digest, so the rows in hand are the count.
  it("counts the calls in hand when no digest reaches the run", () => {
    const group = paint({ items: [toolCall(1, "Read a.js"), reasoning(2, "Thinking."), toolCall(3, "Read b.js")] });

    expect(countOn(group)).toBe("2");
    expect(previewOn(group)).toBe("Read b.js");
  });

  it("keeps the old look for a run that called no tool", () => {
    const group = paint({
      items: [reasoning(1, "Thinking."), reasoning(2, "Still thinking.")],
      activityDigests: [digest(1, 2, 0, null)],
    });

    expect(countOn(group)).toBe("2");
    expect(previewOn(group)).toBe("Still thinking.");
  });
});
