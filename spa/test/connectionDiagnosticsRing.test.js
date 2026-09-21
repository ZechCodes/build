// #60 point 4: a report has to carry the failures, not just the recovery.
//
// Zech pasted his diagnostics from the phone and they covered only the last
// session — the one that worked. The three that failed before it, which were the
// whole story, had been pushed out of a hundred-entry ring by the reconnect storm
// they caused. So the ring holds a wake's worth of events, and when it does
// overflow it says so rather than presenting a truncated history as a whole one.

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  DIAGNOSTIC_LIMIT,
  clearConnectionDiagnosticHistory,
  connectionDiagnosticHistory,
  connectionDiagnosticReport,
  recordConnectionDiagnostic,
} from "../src/core/connectionDiagnostics.js";

const fill = (count, event = "state") => {
  for (let at = 0; at < count; at++) recordConnectionDiagnostic(`dev-a:sess-${at}`, event, { at });
};

describe("the connection diagnostic ring", () => {
  beforeEach(() => clearConnectionDiagnosticHistory());

  it("holds far more than a reconnect storm produces", () => {
    // The storm that hid Zech's evidence was a few hundred entries across four
    // sessions: candidates, ICE states, channel opens and closes, per attempt.
    expect(DIAGNOSTIC_LIMIT).toBeGreaterThanOrEqual(1000);
  });

  it("keeps every event while it is under the limit", () => {
    fill(500);
    expect(connectionDiagnosticHistory()).toHaveLength(500);
    expect(connectionDiagnosticReport().dropped).toBe(0);
  });

  it("drops the oldest when it overflows, and says how many", () => {
    fill(DIAGNOSTIC_LIMIT + 25);
    const report = connectionDiagnosticReport();

    expect(report.events).toHaveLength(DIAGNOSTIC_LIMIT);
    expect(report.dropped).toBe(25);
    // The oldest kept is the 26th recorded: the ring drops from the front.
    expect(report.events[0].connection).toBe("dev-a:sess-25");
  });

  it("says when the page loaded, so a gap in the record is legible", () => {
    const report = connectionDiagnosticReport();
    expect(typeof report.since).toBe("number");
    expect(report.since).toBeLessThanOrEqual(Date.now());
  });

  it("carries the session id on every entry that belongs to one", () => {
    recordConnectionDiagnostic("dev-a:sess-vwlhpi1t", "carrying", { path: "direct" });
    const [entry] = connectionDiagnosticReport().events;
    expect(entry.connection).toBe("dev-a:sess-vwlhpi1t");
    expect(entry.event).toBe("carrying");
    expect(entry.path).toBe("direct");
  });

  it("is cleared as a whole, dropped count included", () => {
    fill(DIAGNOSTIC_LIMIT + 10);
    expect(connectionDiagnosticReport().dropped).toBe(10);

    clearConnectionDiagnosticHistory();

    expect(connectionDiagnosticReport()).toMatchObject({ dropped: 0, events: [] });
  });

  it("stamps each entry with when it happened", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-09-20T23:18:11.410Z"));
      recordConnectionDiagnostic("dev-a:sess-vwlhpi1t", "channel", { channel: "term", state: "closed" });
      expect(connectionDiagnosticHistory()[0].at).toBe(Date.parse("2026-09-20T23:18:11.410Z"));
    } finally {
      vi.useRealTimers();
    }
  });
});
