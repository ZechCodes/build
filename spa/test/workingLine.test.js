// @vitest-environment jsdom
// Working time is branch/issue-level status: it belongs to the toolbar, where
// it is true wherever you are standing in the work, and not to the conversation
// — a timeline that narrates the clock buries what was actually said.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

const userMsg = (over = {}) => ({
  type: "message",
  data: { id: "m1", role: "user", body: "do the thing", created_at: "2026-08-08T03:00:00Z", ...over },
});

describe("the conversation and the clock", () => {
  it("says nothing about working time, however long the agent has had the message", () => {
    const out = threadHtml({ items: [userMsg({ seen_at: "2026-08-08T03:01:00Z" })] });
    expect(out).not.toContain("thread-working");
    expect(out).toContain('aria-label="Read"');
  });
});
