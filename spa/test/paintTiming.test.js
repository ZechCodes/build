// A paint that takes longer than a frame budget should say so, in the console
// and in the performance timeline, without changing what the paint returns.

import { describe, it, expect, vi, afterEach } from "vitest";
import { SLOW_PAINT_MS, timedPaint } from "../src/core/paintTiming.js";

const realPerformance = globalThis.performance;

/** A performance object whose clock the test drives: each `now()` answers the
 *  next queued reading, so a paint can be made to take any duration. */
function fakePerformance(durations) {
  let elapsed = 0;
  const queue = [...durations];
  return {
    marks: [],
    measures: [],
    cleared: [],
    now() {
      const reading = elapsed;
      elapsed += queue.length ? queue.shift() : 0;
      return reading;
    },
    mark(name) {
      this.marks.push(name);
    },
    measure(name, from, to) {
      this.measures.push({ name, from, to });
    },
    clearMarks(name) {
      this.cleared.push(name);
    },
    clearMeasures(name) {
      this.cleared.push(name);
    },
  };
}

const withPerformance = (clock) => {
  globalThis.performance = clock;
  return clock;
};

afterEach(() => {
  globalThis.performance = realPerformance;
  vi.restoreAllMocks();
});

describe("timedPaint", () => {
  it("answers what the paint answers", () => {
    withPerformance(fakePerformance([1]));
    expect(timedPaint("changes", () => "painted")).toBe("painted");
  });

  it("marks and measures the paint under its own name", () => {
    const clock = withPerformance(fakePerformance([1]));
    timedPaint("changes", () => null);
    expect(clock.marks).toEqual(["build:changes:start", "build:changes:end"]);
    expect(clock.measures).toEqual([{ name: "build:changes", from: "build:changes:start", to: "build:changes:end" }]);
  });

  it("clears its marks so a polling surface cannot grow the timeline buffer", () => {
    const clock = withPerformance(fakePerformance([1]));
    timedPaint("changes", () => null);
    expect(clock.cleared).toContain("build:changes:start");
    expect(clock.cleared).toContain("build:changes:end");
    expect(clock.cleared).toContain("build:changes");
  });

  it("warns, naming the paint and its duration, past the threshold", () => {
    withPerformance(fakePerformance([SLOW_PAINT_MS + 1]));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    timedPaint("changes", () => null);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0].join(" ")).toContain("changes");
    expect(warn.mock.calls[0].join(" ")).toContain(String(SLOW_PAINT_MS + 1));
  });

  it("stays quiet at the threshold", () => {
    withPerformance(fakePerformance([SLOW_PAINT_MS]));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    timedPaint("changes", () => null);
    expect(warn).not.toHaveBeenCalled();
  });

  it("lets a failing paint throw, having marked its end", () => {
    const clock = withPerformance(fakePerformance([1]));
    expect(() => timedPaint("changes", () => {
      throw new Error("render blew up");
    })).toThrow("render blew up");
    expect(clock.marks).toEqual(["build:changes:start", "build:changes:end"]);
  });

  it("still paints where the browser has no performance timeline", () => {
    globalThis.performance = undefined;
    expect(timedPaint("changes", () => "painted")).toBe("painted");
  });
});
