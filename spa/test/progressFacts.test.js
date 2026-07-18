import { describe, it, expect } from "vitest";
import { runProgressFacts } from "../src/core/progressFacts.js";

const NOW = Date.parse("2026-07-18T12:00:00Z");
const minutesAgo = (minutes) => new Date(NOW - minutes * 60_000).toISOString();

const fullRun = () => ({
  state: "building",
  stat: { files_changed: 3, insertions: 42, deletions: 7 },
  updated_at: minutesAgo(5),
  state_changed_at: minutesAgo(12),
});

describe("runProgressFacts", () => {
  it("renders every part in order for a full payload", () => {
    expect(runProgressFacts(fullRun(), NOW)).toBe("3 files · +42 −7 · active 5m ago · building for 12m");
  });

  it("uses the singular for one file", () => {
    const run = { ...fullRun(), stat: { files_changed: 1, insertions: 2, deletions: 0 } };
    expect(runProgressFacts(run, NOW)).toBe("1 file · +2 −0 · active 5m ago · building for 12m");
  });

  it("drops the diff part when stat is null", () => {
    const run = { ...fullRun(), stat: null };
    expect(runProgressFacts(run, NOW)).toBe("active 5m ago · building for 12m");
  });

  it("drops the diff part when no files changed", () => {
    const run = { ...fullRun(), stat: { files_changed: 0, insertions: 0, deletions: 0 } };
    expect(runProgressFacts(run, NOW)).toBe("active 5m ago · building for 12m");
  });

  it("drops time-in-state when state_changed_at is missing (older bridge)", () => {
    const run = { ...fullRun(), state_changed_at: undefined };
    expect(runProgressFacts(run, NOW)).toBe("3 files · +42 −7 · active 5m ago");
  });

  it("drops the active part when updated_at is missing", () => {
    const run = { ...fullRun(), updated_at: undefined };
    expect(runProgressFacts(run, NOW)).toBe("3 files · +42 −7 · building for 12m");
  });

  it("speaks human words for gate/review states", () => {
    const run = { ...fullRun(), state: "review" };
    expect(runProgressFacts(run, NOW)).toContain("in review for 12m");
    const gated = { ...fullRun(), state: "stage_gate" };
    expect(runProgressFacts(gated, NOW)).toContain("at stage gate for 12m");
  });

  it("shows <1m for a state entered moments ago", () => {
    const run = { ...fullRun(), state_changed_at: new Date(NOW - 10_000).toISOString() };
    expect(runProgressFacts(run, NOW)).toContain("building for <1m");
  });

  it("is empty for terminal runs", () => {
    for (const state of ["merged", "abandoned", "archived"]) {
      expect(runProgressFacts({ ...fullRun(), state }, NOW)).toBe("");
    }
  });

  it("degrades to empty when nothing is known", () => {
    expect(runProgressFacts({ state: "building", stat: null }, NOW)).toBe("");
  });
});
