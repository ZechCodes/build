// The done event carries the agent's handoff: the files that hold the change,
// what it decided, what could break, what it left alone. It reads as a card in
// the conversation — the thing a reviewer reads before the diff.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

const doneEvent = (report) => ({
  type: "event",
  data: {
    event: "done",
    summary: "built the thing",
    created_at: "2026-08-08T03:00:00Z",
    completion_report: report,
  },
});

const REPORT = {
  critical_files: ["src/rail.js — the strip and the panel"],
  risk_notes: ["the TUI toggle is untested on touch"],
  decisions: ["kept the theme"],
  skips: ["did not touch the console"],
};

describe("the completion report card", () => {
  it("renders the report's headings and lines under the done event", () => {
    const out = threadHtml({ items: [doneEvent(REPORT)] });
    expect(out).toContain("completion-report");
    expect(out).toContain("Critical files");
    expect(out).toContain("src/rail.js — the strip and the panel");
    expect(out).toContain("Decisions");
    expect(out).toContain("Risks");
    expect(out).toContain("Skipped");
  });

  it("escapes what the agent wrote", () => {
    const out = threadHtml({ items: [doneEvent({ decisions: ["<img src=x onerror=alert(1)>"] })] });
    expect(out).not.toContain("<img src=x");
    expect(out).toContain("&lt;img");
  });

  it("leaves out a list the agent left out", () => {
    const out = threadHtml({ items: [doneEvent({ decisions: ["one call"] })] });
    expect(out).toContain("Decisions");
    expect(out).not.toContain("Risks");
  });

  it("is absent when the agent reported done without one", () => {
    const out = threadHtml({ items: [doneEvent(null)] });
    expect(out).toContain("reported done");
    expect(out).not.toContain("completion-report");
  });
});
