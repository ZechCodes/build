import { describe, expect, it } from "vitest";
import { SESSION_GAP_MS, sessionTimes } from "../src/core/sessionSpans.js";

describe("bridge session summaries", () => {
  it("reads the bridge's exact anchor and last activity", () => {
    expect(SESSION_GAP_MS).toBe(12 * 60 * 60 * 1000);
    expect(sessionTimes({ session_started_ms: 100, last_activity_ms: 200 })).toEqual({
      anchorMs: 100, lastActivityMs: 200,
    });
  });

  it("anchors old bridges today", () => {
    for (const record of [null, {},
      { session_started_ms: 200, last_activity_ms: 100 }]) {
      expect(sessionTimes(record, 300)).toEqual({ anchorMs: 300, lastActivityMs: 300 });
    }
  });

  it("anchors a new bridge's empty workspace at creation", () => {
    expect(sessionTimes({ session_started_ms: null, last_activity_ms: null,
      created_at: "2026-08-01T00:00:00Z" }, 300)).toEqual({
      anchorMs: Date.parse("2026-08-01T00:00:00Z"),
      lastActivityMs: Date.parse("2026-08-01T00:00:00Z"),
    });
  });
});
