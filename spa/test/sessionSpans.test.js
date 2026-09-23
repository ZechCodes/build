import { describe, expect, it } from "vitest";
import { SESSION_GAP_MS, newestSession } from "../src/core/sessionSpans.js";

const HOUR = 60 * 60 * 1000;
const conversation = (...activity_spans) => ({ activity_spans });

describe("pooled conversation sessions", () => {
  it("splits at exactly twelve hours and merges a gap just under it", () => {
    expect(newestSession([conversation([0, 0], [SESSION_GAP_MS, SESSION_GAP_MS])])).toEqual({
      anchorMs: SESSION_GAP_MS, lastActivityMs: SESSION_GAP_MS,
    });
    expect(newestSession([conversation([0, 0]), conversation([SESSION_GAP_MS - 1, SESSION_GAP_MS - 1])])).toEqual({
      anchorMs: 0, lastActivityMs: SESSION_GAP_MS - 1,
    });
  });

  it("uses the newest of several gaps and can join spans through another conversation", () => {
    expect(newestSession([
      conversation([42 * HOUR, 42 * HOUR], [16 * HOUR, 16 * HOUR], [0, 0]),
      conversation([31 * HOUR, 31 * HOUR], [21 * HOUR, 21 * HOUR]),
    ])).toEqual({ anchorMs: 16 * HOUR, lastActivityMs: 42 * HOUR });
  });

  it("distinguishes no messages from an older bridge that supplied no spans", () => {
    expect(newestSession([])).toEqual({ anchorMs: null, lastActivityMs: null });
    expect(newestSession([conversation()])).toEqual({ anchorMs: null, lastActivityMs: null });
    expect(newestSession()).toBeNull();
    expect(newestSession([{}])).toBeNull();
  });
});
