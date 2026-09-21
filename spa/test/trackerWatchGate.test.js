// The watch gate (#65): does this machine's bridge know what watching is?
//
// Same shape as #21's issue-context gate, and refused by default for the same
// reason — a verb a bridge has never heard of is a refusal, not a polite no,
// and the read mark would produce one per glance at an issue.

import { describe, it, expect, vi, beforeEach } from "vitest";

const version = vi.fn();
vi.mock("../src/core/changeEvents.js", () => ({ bridgeApiVersion: (...args) => version(...args) }));

const { WATCH_SINCE, carriesWatching } = await import("../src/core/trackerWatch.js");

beforeEach(() => version.mockReset());

describe("when watching is offered", () => {
  it("lands at the minor #64 named, not one guessed", () => {
    expect(WATCH_SINCE).toBe("1.9.0");
  });

  it("is offered at that minor and above", () => {
    for (const said of ["1.9.0", "1.9.1", "1.10.0", "2.0.0"]) {
      version.mockReturnValue(said);
      expect([said, carriesWatching("dev-1")]).toEqual([said, true]);
    }
  });

  // 1.8.0 is the roll BEFORE watching — the exact bridge a guessed-low gate
  // would have shipped a refusing switch onto.
  it("is refused below it, including the roll just before", () => {
    for (const said of ["1.8.0", "1.5.0", "0.9.0"]) {
      version.mockReturnValue(said);
      expect([said, carriesWatching("dev-1")]).toEqual([said, false]);
    }
  });
});

describe("what it does when it cannot tell", () => {
  it("says no to every unknown", () => {
    version.mockReturnValue("1.9.0");
    expect(carriesWatching(null)).toBe(false);
    expect(carriesWatching("dev-1", "")).toBe(false);

    version.mockReturnValue(null);
    expect(carriesWatching("dev-1")).toBe(false);

    version.mockReturnValue("not a version");
    expect(carriesWatching("dev-1")).toBe(false);
  });
});
